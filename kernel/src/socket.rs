//! Linux sockets: AF_INET (over the kernel TCP stack) and AF_UNIX socketpairs.

use crate::errno::*;
use crate::fs::file::*;
use crate::fs::pipe::Pipe;
use crate::net::{self, TcpHandle, TcpState};
use crate::sync::SpinLock;
use crate::task::process::Process;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const AF_UNIX: u16 = 1;
pub const AF_INET: u16 = 2;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_NONBLOCK: u32 = 0x800;
pub const SOCK_CLOEXEC: u32 = 0x80000;

pub enum SockInner {
    Unconnected { domain: u16, ty: u32 },
    UnixPair { rx: Arc<Pipe>, tx: Arc<Pipe> },
    InetIdle,
    InetListening { handle: TcpHandle },
    InetConnected { handle },
}

pub struct Socket {
    pub inner: SpinLock<SockInner>,
}

impl Socket {
    pub fn new_unconnected(domain: u16, ty: u32) -> Arc<Self> {
        Arc::new(Self {
            inner: SpinLock::new(SockInner::Unconnected { domain, ty }),
        })
    }

    pub fn new_inet() -> Arc<Self> {
        Arc::new(Self {
            inner: SpinLock::new(SockInner::InetIdle),
        })
    }

    pub fn new_unix_pair(rx: Arc<Pipe>, tx: Arc<Pipe>) -> Arc<Self> {
        Arc::new(Self {
            inner: SpinLock::new(SockInner::UnixPair { rx, tx }),
        })
    }

    pub fn new_connected(handle: TcpHandle) -> Arc<Self> {
        Arc::new(Self {
            inner: SpinLock::new(SockInner::InetConnected { handle }),
        })
    }

    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let inner = self.inner.lock();
        match &*inner {
            SockInner::UnixPair { rx, .. } => rx.read(buf),
            SockInner::InetConnected { handle } => {
                net::poll();
                match net::tcp_recv(*handle, buf) {
                    Ok(n) => Ok(n),
                    Err(net::ErrnoKind::Again) => Err(EAGAIN),
                    Err(net::ErrnoKind::Closed) => Ok(0),
                    Err(_) => Err(EIO),
                }
            }
            _ => Err(ENOTCONN),
        }
    }

    pub fn send(&self, buf: &[u8]) -> Result<usize, Errno> {
        let inner = self.inner.lock();
        match &*inner {
            SockInner::UnixPair { tx, .. } => tx.write(buf),
            SockInner::InetConnected { handle } => {
                net::poll();
                match net::tcp_send(*handle, buf) {
                    Ok(n) => Ok(n),
                    Err(net::ErrnoKind::Again) => Err(EAGAIN),
                    Err(net::ErrnoKind::Closed) => Err(EPIPE),
                    Err(_) => Err(EIO),
                }
            }
            _ => Err(ENOTCONN),
        }
    }

    pub fn readiness(&self) -> u32 {
        let inner = self.inner.lock();
        match &*inner {
            SockInner::UnixPair { rx, tx } => {
                let mut r = 0;
                if rx.readable() {
                    r |= EPOLLIN;
                }
                if tx.writable() {
                    r |= EPOLLOUT;
                }
                r
            }
            SockInner::InetConnected { handle } => {
                net::poll();
                let mut r = 0;
                if net::tcp_pending(*handle) > 0 {
                    r |= EPOLLIN;
                }
                match net::tcp_state(*handle) {
                    TcpState::Established => r |= EPOLLOUT,
                    TcpState::CloseWait | TcpState::Closed | TcpState::LastAck => {
                        r |= EPOLLIN | EPOLLHUP
                    }
                    TcpState::TimeWait => r |= EPOLLHUP,
                    _ => {}
                }
                r
            }
            SockInner::InetListening { .. } => {
                net::poll();
                EPOLLIN
            }
            _ => EPOLLIN | EPOLLOUT,
        }
    }

    pub fn shutdown(&self, how: i32) -> Result<(), Errno> {
        let inner = self.inner.lock();
        if let SockInner::InetConnected { handle } = &*inner {
            net::tcp_shutdown(*handle, how == 0 || how == 2, how == 1 || how == 2);
        }
        Ok(())
    }

    pub fn local_addr(&self) -> Option<([u8; 4], u16)> {
        match &*self.inner.lock() {
            SockInner::InetConnected { handle } | SockInner::InetListening { handle } => {
                net::tcp_local_addr(*handle)
            }
            _ => None,
        }
    }

    pub fn peer_addr(&self) -> Option<([u8; 4], u16)> {
        match &*self.inner.lock() {
            SockInner::InetConnected { handle } => net::tcp_peer_addr(*handle),
            _ => None,
        }
    }

    pub fn is_listening(&self) -> bool {
        matches!(&*self.inner.lock(), SockInner::InetListening { .. })
    }

    pub fn is_connected(&self) -> bool {
        matches!(&*self.inner.lock(), SockInner::InetConnected { .. })
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let mut inner = self.inner.lock();
        match &mut *inner {
            SockInner::InetConnected { handle } | SockInner::InetListening { handle } => {
                net::tcp_drop(*handle);
            }
            _ => {}
        }
    }
}

fn get_sock(p: &mut Process, fd: i32) -> Result<Arc<Socket>, Errno> {
    let f = p.files.get(fd)?;
    match &*f.obj.lock() {
        FileObj::Socket(s) => Ok(s.clone()),
        _ => Err(ENOTSOCK),
    }
}

fn parse_sockaddr(p: &mut Process, addr: usize, len: usize) -> Result<([u8; 4], u16, u16), Errno> {
    if addr == 0 || len < 8 {
        return Err(EINVAL);
    }
    let mut b = [0u8; 16];
    let n = len.min(16);
    p.copy_from_user(addr, &mut b[..n])?;
    let family = u16::from_le_bytes(b[0..2].try_into().unwrap());
    if family != AF_INET {
        return Ok(([0, 0, 0, 0], 0, family));
    }
    let port = u16::from_be_bytes(b[2..4].try_into().unwrap());
    let ip = [b[4], b[5], b[6], b[7]];
    Ok((ip, port, family))
}

fn write_sockaddr(
    p: &mut Process,
    addr: usize,
    addrlen: usize,
    ip: [u8; 4],
    port: u16,
) -> Result<(), Errno> {
    if addr == 0 || addrlen == 0 {
        return Ok(());
    }
    let mut b = [0u8; 16];
    b[0..2].copy_from_slice(&AF_INET.to_le_bytes());
    b[2..4].copy_from_slice(&port.to_be_bytes());
    b[4..8].copy_from_slice(&ip);
    p.copy_to_user(addr, &b)?;
    p.copy_to_user(addrlen, &16u32.to_le_bytes())?;
    Ok(())
}

pub fn sys_socket(p: &mut Process, domain: usize, ty: u32, _proto: u32) -> Result<usize, Errno> {
    let base_ty = ty & 0xf;
    let sock = match domain as u16 {
        AF_INET => Socket::new_inet(),
        AF_UNIX => Socket::new_unconnected(AF_UNIX, base_ty),
        _ => return Err(EAFNOSUPPORT),
    };
    if base_ty != SOCK_STREAM {
        return Err(EPROTONOSUPPORT);
    }
    let f = File::new(
        FileObj::Socket(sock),
        if ty & SOCK_NONBLOCK != 0 {
            O_NONBLOCK
        } else {
            0
        },
        ty & SOCK_CLOEXEC != 0,
    );
    Ok(p.files.alloc(f, 0)? as usize)
}

pub fn sys_socketpair(
    p: &mut Process,
    domain: usize,
    ty: u32,
    _proto: u32,
    sv: usize,
) -> Result<usize, Errno> {
    if domain as u16 != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let a2b = Pipe::new();
    let b2a = Pipe::new();
    let s0 = Socket::new_unix_pair(a2b.clone(), b2a.clone());
    let s1 = Socket::new_unix_pair(b2a, a2b);
    let flags = if ty & SOCK_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    let cloexec = ty & SOCK_CLOEXEC != 0;
    let f0 = File::new(FileObj::Socket(s0), flags, cloexec);
    let f1 = File::new(FileObj::Socket(s1), flags, cloexec);
    let fd0 = p.files.alloc(f0, 0)?;
    let fd1 = p.files.alloc(f1, 0)?;
    let mut b = [0u8; 8];
    b[0..4].copy_from_slice(&(fd0 as u32).to_le_bytes());
    b[4..8].copy_from_slice(&(fd1 as u32).to_le_bytes());
    p.copy_to_user(sv, &b)?;
    Ok(0)
}

pub fn sys_bind(p: &mut Process, fd: i32, addr: usize, len: usize) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let (ip, port, family) = parse_sockaddr(p, addr, len)?;
    if family != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let _ = ip;
    let handle = net::tcp_listen(port).map_err(|_| EADDRINUSE)?;
    *s.inner.lock() = SockInner::InetListening { handle };
    Ok(0)
}

pub fn sys_listen(p: &mut Process, fd: i32, _backlog: i32) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    if !s.is_listening() {
        let handle = net::tcp_listen(0).map_err(|_| EADDRINUSE)?;
        *s.inner.lock() = SockInner::InetListening { handle };
    }
    Ok(0)
}

pub fn sys_accept(
    p: &mut Process,
    fd: i32,
    addr: usize,
    addrlen: usize,
    nonblock: bool,
) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let handle = match &*s.inner.lock() {
        SockInner::InetListening { handle } => *handle,
        _ => return Err(EINVAL),
    };
    let f = p.files.get(fd)?;
    loop {
        net::poll();
        match net::tcp_accept(handle) {
            Ok(Some((nh, ip, port))) => {
                if addr != 0 {
                    write_sockaddr(p, addr, addrlen, ip, port)?;
                }
                let ns = Socket::new_connected(nh);
                let nf = File::new(
                    FileObj::Socket(ns),
                    if nonblock { O_NONBLOCK } else { 0 },
                    false,
                );
                return Ok(p.files.alloc(nf, 0)? as usize);
            }
            Ok(None) => {
                if nonblock || f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(_) => return Err(EINVAL),
        }
    }
}

pub fn sys_connect(p: &mut Process, fd: i32, addr: usize, len: usize) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let (ip, port, family) = parse_sockaddr(p, addr, len)?;
    if family == AF_UNIX {
        return Err(ENOENT);
    }
    if family != AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let handle = net::tcp_connect(ip, port).map_err(|_| ENETUNREACH)?;
    *s.inner.lock() = SockInner::InetConnected { handle };
    let f = p.files.get(fd)?;
    loop {
        net::poll();
        match net::tcp_state(handle) {
            TcpState::Established => return Ok(0),
            TcpState::Closed | TcpState::TimeWait => return Err(ECONNREFUSED),
            _ => {}
        }
        if f.nonblocking() {
            return Err(EINPROGRESS);
        }
        if p.sig.pending & !p.sig.blocked != 0 {
            return Err(EINTR);
        }
        crate::task::sleep_ticks(1);
    }
}

pub fn sys_getsockname(
    p: &mut Process,
    fd: i32,
    addr: usize,
    addrlen: usize,
) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let (ip, port) = s.local_addr().unwrap_or(([10, 0, 2, 15], 0));
    write_sockaddr(p, addr, addrlen, ip, port)?;
    Ok(0)
}

pub fn sys_getpeername(
    p: &mut Process,
    fd: i32,
    addr: usize,
    addrlen: usize,
) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let (ip, port) = s.peer_addr().ok_or(ENOTCONN)?;
    write_sockaddr(p, addr, addrlen, ip, port)?;
    Ok(0)
}

pub fn sys_sendto(
    p: &mut Process,
    fd: i32,
    buf: usize,
    len: usize,
    _flags: usize,
    _addr: usize,
    _alen: usize,
) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let mut data = alloc::vec![0u8; len.min(1 << 20)];
    p.copy_from_user(buf, &mut data)?;
    let f = p.files.get(fd)?;
    loop {
        match s.send(&data) {
            Ok(n) => return Ok(n),
            Err(EAGAIN) => {
                if f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn sys_recvfrom(
    p: &mut Process,
    fd: i32,
    buf: usize,
    len: usize,
    _flags: usize,
    addr: usize,
    addrlen: usize,
) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let mut data = alloc::vec![0u8; len.min(1 << 20)];
    let f = p.files.get(fd)?;
    loop {
        match s.recv(&mut data) {
            Ok(n) => {
                p.copy_to_user(buf, &data[..n])?;
                if addr != 0 {
                    if let Some((ip, port)) = s.peer_addr() {
                        write_sockaddr(p, addr, addrlen, ip, port)?;
                    }
                }
                return Ok(n);
            }
            Err(EAGAIN) => {
                if f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(e) => return Err(e),
        }
    }
}

fn read_msghdr(p: &mut Process, hdr: usize) -> Result<Vec<(usize, usize)>, Errno> {
    let mut b = [0u8; 56];
    p.copy_from_user(hdr, &mut b)?;
    let iov = u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize;
    let iovlen = u64::from_le_bytes(b[24..32].try_into().unwrap()) as usize;
    let mut iovs = Vec::new();
    for i in 0..iovlen.min(1024) {
        let mut ib = [0u8; 16];
        p.copy_from_user(iov + i * 16, &mut ib)?;
        let base = u64::from_le_bytes(ib[0..8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(ib[8..16].try_into().unwrap()) as usize;
        iovs.push((base, len));
    }
    Ok(iovs)
}

pub fn sys_sendmsg(p: &mut Process, fd: i32, hdr: usize, _flags: usize) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let iovs = read_msghdr(p, hdr)?;
    let mut data = Vec::new();
    for (base, len) in iovs.iter() {
        let mut b = alloc::vec![0u8; *len];
        p.copy_from_user(*base, &mut b)?;
        data.extend_from_slice(&b);
    }
    let f = p.files.get(fd)?;
    loop {
        match s.send(&data) {
            Ok(n) => return Ok(n),
            Err(EAGAIN) => {
                if f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn sys_recvmsg(p: &mut Process, fd: i32, hdr: usize, _flags: usize) -> Result<usize, Errno> {
    let s = get_sock(p, fd)?;
    let iovs = read_msghdr(p, hdr)?;
    let total: usize = iovs.iter().map(|(_, l)| *l).sum();
    if total == 0 {
        return Ok(0);
    }
    let mut data = alloc::vec![0u8; total.min(1 << 20)];
    let f = p.files.get(fd)?;
    let n = loop {
        match s.recv(&mut data) {
            Ok(n) => break n,
            Err(EAGAIN) => {
                if f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(e) => return Err(e),
        }
    };
    let mut off = 0;
    for (base, len) in iovs.iter() {
        if off >= n {
            break;
        }
        let k = (*len).min(n - off);
        p.copy_to_user(*base, &data[off..off + k])?;
        off += k;
    }
    p.copy_to_user(hdr + 8, &0u32.to_le_bytes())?;
    p.copy_to_user(hdr + 48, &0i32.to_le_bytes())?;
    Ok(n)
}

pub fn sys_sendmmsg(
    p: &mut Process,
    fd: i32,
    mmsghdr: usize,
    vlen: usize,
    flags: usize,
) -> Result<usize, Errno> {
    let mut done = 0;
    for i in 0..vlen {
        match sys_sendmsg(p, fd, mmsghdr + i * 64, flags) {
            Ok(n) => {
                p.copy_to_user(mmsghdr + i * 64 + 56, &(n as u32).to_le_bytes())?;
                done += 1;
            }
            Err(_) if done > 0 => break,
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

pub fn sys_recvmmsg(
    p: &mut Process,
    fd: i32,
    mmsghdr: usize,
    vlen: usize,
    flags: usize,
    _timeout: usize,
) -> Result<usize, Errno> {
    let mut done = 0;
    for i in 0..vlen {
        match sys_recvmsg(p, fd, mmsghdr + i * 64, flags) {
            Ok(n) => {
                p.copy_to_user(mmsghdr + i * 64 + 56, &(n as u32).to_le_bytes())?;
                done += 1;
            }
            Err(_) if done > 0 => break,
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

pub fn sys_getsockopt(
    p: &mut Process,
    fd: i32,
    _level: usize,
    _optname: usize,
    optval: usize,
    optlen: usize,
) -> Result<usize, Errno> {
    let _ = get_sock(p, fd)?;
    if optlen != 0 {
        let mut b = [0u8; 4];
        p.copy_from_user(optlen, &mut b)?;
        let n = u32::from_le_bytes(b) as usize;
        let z = alloc::vec![0u8; n.min(64)];
        if optval != 0 {
            p.copy_to_user(optval, &z)?;
        }
    }
    Ok(0)
}

pub fn sys_shutdown(p: &mut Process, fd: i32, how: i32) -> Result<(), Errno> {
    let s = get_sock(p, fd)?;
    s.shutdown(how)
}
