//! Socket/epoll syscalls are implemented in crate::net.
pub use crate::net::{
    accept4, bind, connect, epoll_create1, epoll_ctl, epoll_pwait, getpeername, getsockname,
    getsockopt, listen, ppoll, recvfrom, recvmsg, sendmsg, sendto, setsockopt, shutdown, socket,
    socketpair,
};
