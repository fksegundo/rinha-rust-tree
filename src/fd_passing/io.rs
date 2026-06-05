use crate::http::BUF_SIZE;
use std::io;
use std::os::fd::RawFd;

pub enum ReadOutcome {
    Data(usize),
    WouldBlock,
    Closed,
}

pub fn greedy_read(client_fd: RawFd, buf: &mut [u8; BUF_SIZE], mut used: usize) -> ReadOutcome {
    let had_data = used > 0;
    loop {
        let n = greedy_read_into(client_fd, buf, used);
        if n > 0 {
            used += n as usize;
            if used >= BUF_SIZE {
                return ReadOutcome::Data(used);
            }
            continue;
        }

        if n == 0 {
            return if used > 0 || had_data {
                ReadOutcome::Data(used)
            } else {
                ReadOutcome::Closed
            };
        }

        let err = io::Error::last_os_error();
        return if err.raw_os_error() == Some(libc::EAGAIN) {
            if used > 0 || had_data {
                ReadOutcome::Data(used)
            } else {
                ReadOutcome::WouldBlock
            }
        } else {
            ReadOutcome::Closed
        };
    }
}

fn greedy_read_into(client_fd: RawFd, buf: &mut [u8; BUF_SIZE], used: usize) -> isize {
    if used >= BUF_SIZE {
        return 0;
    }
    unsafe {
        libc::read(
            client_fd,
            buf.as_mut_ptr().add(used) as *mut libc::c_void,
            BUF_SIZE - used,
        )
    }
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn set_tcp_nodelay(fd: RawFd) {
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_QUICKACK,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

pub fn bind_seqpacket_listener(socket_path: &str) -> io::Result<RawFd> {
    let _ = std::fs::remove_file(socket_path);
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let result = (|| {
        let addr = unix_sockaddr(socket_path)?;
        let rc = unsafe {
            libc::bind(
                fd,
                &addr.storage as *const _ as *const libc::sockaddr,
                addr.len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let _ = std::fs::set_permissions(
            socket_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o777),
        );
        if unsafe { libc::listen(fd, 4) } != 0 {
            return Err(io::Error::last_os_error());
        }
        set_nonblocking(fd)?;
        Ok(())
    })();

    if let Err(err) = result {
        unsafe { libc::close(fd) };
        return Err(err);
    }
    Ok(fd)
}

struct UnixSockAddr {
    storage: libc::sockaddr_un,
    len: libc::socklen_t,
}

fn unix_sockaddr(path: &str) -> io::Result<UnixSockAddr> {
    let bytes = path.as_bytes();
    if bytes.is_empty() || bytes.len() >= 108 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unix socket path is empty or too long",
        ));
    }
    let mut storage: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    storage.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in storage.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *dst = src as libc::c_char;
    }
    Ok(UnixSockAddr {
        storage,
        len: (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t,
    })
}

pub fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}
