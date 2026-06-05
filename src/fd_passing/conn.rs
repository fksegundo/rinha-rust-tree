use crate::http::BUF_SIZE;
use std::os::fd::RawFd;

const MAX_CLIENT_FD: usize = 65_536;
const MAX_PIPELINED_RESPONSES: usize = 64;

pub struct ConnTable {
    pub slots: Vec<Option<ConnState>>,
    /// Last `EPOLLIN` / `EPOLLOUT` registered for each fd (`None` = not in epoll).
    pub epoll_interest: Vec<Option<i32>>,
    buf_pool: Vec<Box<[u8; BUF_SIZE]>>,
    buf_pool_cap: usize,
}

impl ConnTable {
    pub fn new() -> Self {
        let buf_pool_cap = std::env::var("RINHA_BUF_POOL_INIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(512);

        // Pre-allocate the full FD index to avoid Vec resize/realloc during the benchmark.
        // This reduces tail outliers caused by occasional large FD values.
        let mut slots: Vec<Option<ConnState>> = Vec::with_capacity(MAX_CLIENT_FD);
        slots.resize_with(MAX_CLIENT_FD, || None);

        let mut epoll_interest: Vec<Option<i32>> = Vec::with_capacity(MAX_CLIENT_FD);
        epoll_interest.resize_with(MAX_CLIENT_FD, || None);

        let mut buf_pool = Vec::with_capacity(buf_pool_cap);
        for _ in 0..buf_pool_cap {
            buf_pool.push(Box::new([0u8; BUF_SIZE]));
        }

        Self {
            slots,
            epoll_interest,
            buf_pool,
            buf_pool_cap,
        }
    }

    #[inline(always)]
    pub fn epoll_interest(&self, fd: RawFd) -> Option<i32> {
        self.epoll_interest.get(fd as usize).copied().flatten()
    }

    #[inline(always)]
    pub fn set_epoll_interest(&mut self, fd: RawFd, interest: Option<i32>) {
        let idx = fd as usize;
        if idx < self.epoll_interest.len() {
            self.epoll_interest[idx] = interest;
        }
    }

    #[inline(always)]
    pub fn alloc_buf(&mut self) -> Box<[u8; BUF_SIZE]> {
        self.buf_pool
            .pop()
            .unwrap_or_else(|| Box::new([0u8; BUF_SIZE]))
    }

    #[inline(always)]
    pub fn recycle_buf(&mut self, buf: Box<[u8; BUF_SIZE]>) {
        if self.buf_pool_cap > 0 && self.buf_pool.len() < self.buf_pool_cap {
            self.buf_pool.push(buf);
        }
        // else: drop
    }

    #[inline(always)]
    pub fn recycle_conn_state_buf(&mut self, state: ConnState) {
        match state {
            ConnState::Reading { buf, .. } => self.recycle_buf(buf),
            ConnState::Writing { buf, .. } => self.recycle_buf(buf),
        }
    }

    pub fn insert(&mut self, fd: RawFd, state: ConnState) {
        let idx = fd as usize;
        if idx >= MAX_CLIENT_FD {
            return;
        }
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(state);
    }

    pub fn remove(&mut self, fd: RawFd) -> Option<ConnState> {
        self.slots.get_mut(fd as usize)?.take()
    }
}

#[derive(Clone)]
pub enum ConnState {
    Reading {
        buf: Box<[u8; BUF_SIZE]>,
        used: usize,
    },
    Writing {
        buf: Box<[u8; BUF_SIZE]>,
        responses: ResponseBatch,
        written: usize,
        leftover_off: usize,
        leftover_len: usize,
        keep_alive: bool,
    },
}

#[derive(Clone, Copy)]
pub struct ResponseBatch {
    items: [&'static [u8]; MAX_PIPELINED_RESPONSES],
    len: usize,
}

impl ResponseBatch {
    pub fn new() -> Self {
        Self {
            items: [EMPTY_RESPONSE; MAX_PIPELINED_RESPONSES],
            len: 0,
        }
    }

    pub fn single(response: &'static [u8]) -> Self {
        let mut batch = Self::new();
        let _ = batch.push(response);
        batch
    }

    #[inline(always)]
    pub fn push(&mut self, response: &'static [u8]) -> bool {
        if self.len == self.items.len() {
            return false;
        }
        self.items[self.len] = response;
        self.len += 1;
        true
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[&'static [u8]] {
        &self.items[..self.len]
    }

    #[inline(always)]
    pub fn total_len(&self) -> usize {
        self.as_slice().iter().map(|r| r.len()).sum()
    }
}

const EMPTY_RESPONSE: &'static [u8] = &[];
