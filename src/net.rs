// Networking subsystem.  (Initially stubs; replaced by a smoltcp based stack.)
use alloc::vec::Vec;

pub fn poll() {}

pub fn socket(_domain: i32, _ty: i32, _proto: i32) -> isize {
    -38
}
pub fn close_socket(_id: u32) {}
pub fn bind(_id: u32, _addr: &[u8]) -> isize {
    -38
}
pub fn listen(_id: u32, _backlog: i32) -> isize {
    -38
}
pub fn accept(_id: u32, _flags: u32, _addr_out: &mut [u8]) -> isize {
    -38
}
pub fn connect(_id: u32, _addr: &[u8]) -> isize {
    -38
}
pub fn send(_id: u32, _buf: &[u8]) -> isize {
    -38
}
pub fn recv(_id: u32, _buf: &mut [u8]) -> isize {
    -38
}
pub fn sendto(_id: u32, _buf: &[u8], _addr: &[u8]) -> isize {
    -38
}
pub fn recvfrom(_id: u32, _buf: &mut [u8], _addr_out: &mut [u8]) -> isize {
    -38
}
pub fn shutdown_socket(_id: u32, _how: i32) -> isize {
    -38
}
pub fn getsockname(_id: u32, _out: &mut [u8]) -> isize {
    -38
}
pub fn getpeername(_id: u32, _out: &mut [u8]) -> isize {
    -38
}
pub fn setsockopt(_id: u32, _level: i32, _name: i32, _val: &[u8]) -> isize {
    0
}
pub fn getsockopt(_id: u32, _level: i32, _name: i32, _out: &mut [u8]) -> isize {
    0
}
pub fn pending(_id: u32) -> isize {
    0
}
pub fn epoll_create() -> isize {
    -38
}
pub fn epoll_close(_id: u32) {}
pub fn epoll_ctl(_id: u32, _op: i32, _fd: i32, _event: &[u8]) -> isize {
    -38
}
pub fn epoll_wait(_id: u32, _events: &mut [u8], _max: i32, _timeout: i32) -> isize {
    -38
}
pub fn pipe2(_fds: &mut [u8], _flags: u32) -> isize {
    -38
}
pub fn pipe_read(_id: u32, _buf: &mut [u8]) -> isize {
    -38
}
pub fn pipe_write(_id: u32, _buf: &[u8]) -> isize {
    -38
}
pub fn pipe_close(_id: u32) {}

pub fn _unused(_v: Vec<u8>) {}
