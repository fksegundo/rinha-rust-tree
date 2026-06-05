mod conn;
mod epoll;
mod evented;
mod io;

pub use evented::{run_fd_evented_server, run_fd_evented_server_with_hook};

use std::os::fd::RawFd;

pub(crate) enum RecvFdResult {
    Fd(i32),
    WouldBlock,
    Closed,
}

pub(crate) fn recv_fd_nb(fd: RawFd) -> RecvFdResult {
    let mut buf = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u8; 64];
    let mut msg = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len() as _,
        msg_flags: 0,
    };

    let received =
        unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EAGAIN) {
            return RecvFdResult::WouldBlock;
        }
        return RecvFdResult::Closed;
    }

    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg) as *const i32;
                return RecvFdResult::Fd(*data);
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    RecvFdResult::Closed
}
