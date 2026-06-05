use std::io;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

const MAX_EVENTS: i32 = 256;

#[repr(C)]
pub struct EpollParams {
    pub busy_poll_usecs: u32,
    pub busy_poll_budget: u16,
    pub prefer_busy_poll: u8,
    pub _pad: u8,
}

const EPIOCSPARAMS: libc::c_ulong = 0x40087001;

pub fn configure_epoll_busy_poll(epoll_fd: RawFd) {
    let busy_poll_us: u32 = std::env::var("RINHA_BUSY_POLL_US")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    if busy_poll_us == 0 {
        return;
    }
    let budget: u16 = std::env::var("RINHA_BUSY_POLL_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let prefer: u8 = std::env::var("RINHA_PREFER_BUSY_POLL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let params = EpollParams {
        busy_poll_usecs: busy_poll_us,
        busy_poll_budget: budget,
        prefer_busy_poll: prefer,
        _pad: 0,
    };
    let rc = unsafe { libc::ioctl(epoll_fd, EPIOCSPARAMS, &params) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINVAL | libc::ENOTTY | libc::EOPNOTSUPP) => {}
            _ => {
                eprintln!(
                    "EPIOCSPARAMS failed (busy_poll_us={}): {}",
                    busy_poll_us, err
                );
            }
        }
    }
}

pub fn epoll_idle_us_from_env() -> i64 {
    if let Ok(value) = std::env::var("RINHA_EPOLL_IDLE_US") {
        if let Ok(parsed) = value.parse::<i64>() {
            if parsed >= 0 {
                return parsed;
            }
        }
    }

    std::env::var("RINHA_EPOLL_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .map(|value| value * 1_000)
        .unwrap_or(60)
}

pub fn spin_before_block_us_from_env() -> u32 {
    std::env::var("RINHA_SPIN_BEFORE_BLOCK_US")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn epoll_edge_enabled() -> bool {
    std::env::var("RINHA_EPOLL_EDGE")
        .map(|v| v != "0")
        .unwrap_or(true)
}

pub fn epoll_events(base: i32) -> u32 {
    let mut events = base as u32;
    if epoll_edge_enabled() {
        events |= libc::EPOLLET as u32;
    }
    events
}

pub fn epoll_add(epoll_fd: RawFd, fd: RawFd, token: u64, events: i32) {
    let mut event = libc::epoll_event {
        events: epoll_events(events),
        u64: token,
    };
    if unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut event) } != 0 {
        eprintln!("epoll_ctl ADD failed: {}", io::Error::last_os_error());
    }
}

pub fn epoll_mod(epoll_fd: RawFd, fd: RawFd, events: i32) {
    let mut event = libc::epoll_event {
        events: epoll_events(events),
        u64: fd as u64,
    };
    if unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_MOD, fd, &mut event) } != 0 {
        epoll_add(epoll_fd, fd, fd as u64, events);
    }
}

pub fn epoll_del(epoll_fd: RawFd, fd: RawFd) {
    let _ = unsafe { libc::epoll_ctl(epoll_fd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
}

/// Busy-poll epoll with timeout 0 before blocking, to avoid scheduler wakeup latency.
pub fn epoll_wait_spin_then_block(
    epoll_fd: RawFd,
    events: &mut [libc::epoll_event],
    spin_us: u32,
    block_timeout_us: i64,
) -> i32 {
    let deadline = Instant::now() + Duration::from_micros(spin_us as u64);
    loop {
        let ready = unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, 0) };
        if ready != 0 {
            return ready;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::hint::spin_loop();
    }
    epoll_wait_block(epoll_fd, events, block_timeout_us)
}

pub fn epoll_wait_block(epoll_fd: RawFd, events: &mut [libc::epoll_event], timeout_us: i64) -> i32 {
    let timeout = libc::timespec {
        tv_sec: timeout_us / 1_000_000,
        tv_nsec: (timeout_us % 1_000_000) * 1_000,
    };
    let ready = unsafe {
        libc::epoll_pwait2(
            epoll_fd,
            events.as_mut_ptr(),
            MAX_EVENTS,
            &timeout,
            std::ptr::null(),
        )
    };
    if ready >= 0 {
        return ready;
    }

    let err = io::Error::last_os_error();
    if matches!(err.raw_os_error(), Some(libc::ENOSYS) | Some(libc::EINVAL)) {
        let timeout_ms = ((timeout_us + 999) / 1_000).max(1) as i32;
        return unsafe { libc::epoll_wait(epoll_fd, events.as_mut_ptr(), MAX_EVENTS, timeout_ms) };
    }
    ready
}
