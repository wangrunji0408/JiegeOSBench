//! Socket-related syscalls (AF_INET TCP + AF_UNIX socketpair).

use crate::fs::{Fd, FdKind};
use crate::syscall::{read_cstr, read_user, write_user};
use crate::task;

const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;

fn get_fd(fd: usize) -> Option<&'static mut Fd> {
    let t = task::current();
    unsafe { (&mut t.as_ref().unwrap().fds).get_mut(fd) }
}

fn sock_id_of(fd: &Fd) -> Option<usize> {
    match fd.kind {
        FdKind::Socket { sock_id } | FdKind::UnixPair { sock_id } => Some(sock_id),
        _ => None,
    }
}

pub fn sys_socket(domain: i32, sock_type: i32, protocol: i32) -> isize {
    let _ = protocol;
    if domain != AF_INET && domain != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    if base_type != SOCK_STREAM {
        return -95; // EOPNOTSUPP
    }
    let nonblock = sock_type & SOCK_NONBLOCK != 0;
    let cloexec = sock_type & SOCK_CLOEXEC != 0;
    let id = match crate::net::sock_create(domain, base_type) {
        Ok(id) => id,
        Err(e) => return e as isize,
    };
    crate::net::sock(id).unwrap().nonblock = nonblock;
    let fds = unsafe { &mut *({
        let t = task::current();
        &mut t.as_ref().unwrap().fds as *mut _
    }) };
    let fdnum = match fds.alloc() {
        Some(fd) => fd,
        None => return -24,
    };
    fds.fds[fdnum] = Some(Fd {
        kind: FdKind::Socket { sock_id: id },
        flags: if nonblock { crate::fs::O_NONBLOCK } else { 0 },
        offset: 0,
        cloexec,
        epoll: None,
    });
    fdnum as isize
}

fn parse_sockaddr(addr: usize, len: usize) -> Result<(i32, u32, u16), i32> {
    if len < 8 {
        return Err(-22);
    }
    let d = read_user(addr, 8)?;
    let family = u16::from_le_bytes([d[0], d[1]]) as i32;
    if family == AF_INET {
        if len < 16 {
            return Err(-22);
        }
        let d = read_user(addr, 16)?;
        let port = u16::from_be_bytes([d[2], d[3]]);
        let ip = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
        Ok((family, ip, port))
    } else if family == AF_UNIX {
        Ok((family, 0, 0))
    } else {
        Err(-97)
    }
}

pub fn sys_bind(fd: usize, addr: usize, len: usize) -> isize {
    let (family, ip, port) = match parse_sockaddr(addr, len) {
        Ok(v) => v,
        Err(e) => return e as isize,
    };
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    if family == AF_INET {
        match crate::net::sock_bind(sock_id, ip, port) {
            Ok(_) => 0,
            Err(e) => e as isize,
        }
    } else {
        -22
    }
}

pub fn sys_listen(fd: usize, backlog: i32) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    match crate::net::sock_listen(sock_id, backlog) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_accept(fd: usize, addr: usize, addrlen: usize, flags: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let nonblock = flags & SOCK_NONBLOCK as usize != 0 || crate::net::sock(sock_id).unwrap().nonblock;
    match crate::net::sock_accept(sock_id, nonblock) {
        Ok((new_id, peer_ip, peer_port)) => {
            // write peer address if requested
            if addr != 0 {
                let mut sa = [0u8; 16];
                sa[0] = 2;
                sa[1] = 0;
                sa[2..4].copy_from_slice(&peer_port.to_be_bytes());
                sa[4..8].copy_from_slice(&peer_ip.to_be_bytes());
                // update addrlen to 16
                if addrlen != 0 {
                    let _ = write_user(addrlen, &16u32.to_le_bytes());
                }
                let _ = write_user(addr, &sa);
            }
            // create fd
            let fds = unsafe { &mut *({
                let t = task::current();
                &mut t.as_ref().unwrap().fds as *mut _
            }) };
            let fdnum = match fds.alloc() {
                Some(fd) => fd,
                None => return -24,
            };
            fds.fds[fdnum] = Some(Fd {
                kind: FdKind::Socket { sock_id: new_id },
                flags: if nonblock { crate::fs::O_NONBLOCK } else { 0 },
                offset: 0,
                cloexec: flags & SOCK_CLOEXEC as usize != 0,
                epoll: None,
            });
            fdnum as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_connect(fd: usize, addr: usize, len: usize) -> isize {
    let (family, _ip, _port) = match parse_sockaddr(addr, len) {
        Ok(v) => v,
        Err(e) => return e as isize,
    };
    let _ = family;
    // outbound TCP not supported (nginx only listens); AF_UNIX pathname not supported
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let _ = sock_id;
    -111 // ECONNREFUSED
}

pub fn sys_getsockname(fd: usize, addr: usize, addrlen: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let (ip, port) = crate::net::sock_getsockname(sock_id);
    let mut sa = [0u8; 16];
    sa[0] = 2;
    sa[1] = 0;
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip.to_be_bytes());
    let _ = write_user(addr, &sa);
    if addrlen != 0 {
        let _ = write_user(addrlen, &16u32.to_le_bytes());
    }
    0
}

pub fn sys_getpeername(fd: usize, addr: usize, addrlen: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let (ip, port) = crate::net::sock_getpeername(sock_id);
    let mut sa = [0u8; 16];
    sa[0] = 2;
    sa[1] = 0;
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip.to_be_bytes());
    let _ = write_user(addr, &sa);
    if addrlen != 0 {
        let _ = write_user(addrlen, &16u32.to_le_bytes());
    }
    0
}

pub fn sys_sendto(fd: usize, buf: usize, len: usize, flags: usize, addr: usize, addrlen: usize) -> isize {
    let _ = (flags, addr, addrlen);
    let data = match read_user(buf, len) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match crate::fs::write_fd(f, &data) {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

pub fn sys_recvfrom(fd: usize, buf: usize, len: usize, flags: usize, addr: usize, addrlen: usize) -> isize {
    let _ = (flags, addr, addrlen);
    let mut v = alloc::vec![0u8; len];
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match crate::fs::read_fd(f, &mut v) {
        Ok(n) => {
            if n > 0 {
                let _ = write_user(buf, &v[..n]);
            }
            n as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_setsockopt(fd: usize, level: i32, opt: i32, val: usize, len: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let data = read_user(val, len).unwrap_or_default();
    match crate::net::sock_setsockopt(sock_id, level, opt, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_getsockopt(fd: usize, level: i32, opt: i32, val: usize, vallen: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    let mut out = alloc::vec![0u8; vallen];
    match crate::net::sock_getsockopt(sock_id, level, opt, &mut out) {
        Ok(n) => {
            let _ = write_user(val, &out[..n]);
            // update vallen
            let _ = write_user(vallen, &(n as u32).to_le_bytes());
            0
        }
        Err(e) => e as isize,
    }
}

pub fn sys_shutdown(fd: usize, how: i32) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let sock_id = match sock_id_of(f) {
        Some(id) => id,
        None => return -22,
    };
    match crate::net::sock_shutdown(sock_id, how) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_sendmsg(fd: usize, msg: usize, flags: usize) -> isize {
    let _ = flags;
    // struct msghdr: msg_name(8) msg_namelen(8) msg_iov(8) msg_iovlen(8) msg_control(8) msg_controllen(8) msg_flags(4)
    let hdr = match read_user(msg, 56) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let iov = u64::from_le_bytes(hdr[16..24].try_into().unwrap()) as usize;
    let iovlen = u64::from_le_bytes(hdr[24..32].try_into().unwrap()) as usize;
    let mut total = 0usize;
    for i in 0..iovlen {
        let d = match read_user(iov + i * 16, 16) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let base = u64::from_le_bytes(d[..8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(d[8..].try_into().unwrap()) as usize;
        let data = match read_user(base, len) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let f = match get_fd(fd) {
            Some(f) => f,
            None => return -9,
        };
        match crate::fs::write_fd(f, &data) {
            Ok(n) => total += n,
            Err(e) => return e as isize,
        }
    }
    total as isize
}

pub fn sys_recvmsg(fd: usize, msg: usize, flags: usize) -> isize {
    let _ = flags;
    let hdr = match read_user(msg, 56) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let iov = u64::from_le_bytes(hdr[16..24].try_into().unwrap()) as usize;
    let iovlen = u64::from_le_bytes(hdr[24..32].try_into().unwrap()) as usize;
    let mut total = 0usize;
    for i in 0..iovlen {
        let d = match read_user(iov + i * 16, 16) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let base = u64::from_le_bytes(d[..8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(d[8..].try_into().unwrap()) as usize;
        let mut v = alloc::vec![0u8; len];
        let f = match get_fd(fd) {
            Some(f) => f,
            None => return -9,
        };
        match crate::fs::read_fd(f, &mut v) {
            Ok(n) => {
                if n > 0 {
                    let _ = write_user(base, &v[..n]);
                    total += n;
                }
                if n < len {
                    break;
                }
            }
            Err(e) => return e as isize,
        }
    }
    total as isize
}

pub fn sys_socketpair(domain: i32, sock_type: i32, protocol: i32, sv: usize) -> isize {
    let _ = protocol;
    if domain != AF_UNIX {
        return -97;
    }
    let base_type = sock_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    let nonblock = sock_type & SOCK_NONBLOCK != 0;
    if base_type != SOCK_STREAM {
        return -95;
    }
    let (a, b) = match crate::net::sock_socketpair(base_type) {
        Ok(p) => p,
        Err(e) => return e as isize,
    };
    crate::net::sock(a).unwrap().nonblock = nonblock;
    crate::net::sock(b).unwrap().nonblock = nonblock;
    let fds = unsafe { &mut *({
        let t = task::current();
        &mut t.as_ref().unwrap().fds as *mut _
    }) };
    let fa = match fds.alloc() {
        Some(f) => f,
        None => return -24,
    };
    let fb = match fds.alloc() {
        Some(f) => f,
        None => return -24,
    };
    fds.fds[fa] = Some(Fd {
        kind: FdKind::UnixPair { sock_id: a },
        flags: 0,
        offset: 0,
        cloexec: false,
        epoll: None,
    });
    fds.fds[fb] = Some(Fd {
        kind: FdKind::UnixPair { sock_id: b },
        flags: 0,
        offset: 0,
        cloexec: false,
        epoll: None,
    });
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(fa as u32).to_le_bytes());
    out[4..].copy_from_slice(&(fb as u32).to_le_bytes());
    match write_user(sv, &out) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sock_fd_nonblock(fd: &Fd) -> bool {
    match sock_id_of(fd) {
        Some(id) => crate::net::sock(id).map(|s| s.nonblock).unwrap_or(false),
        None => false,
    }
}

// unused helper guard
pub fn _unused() {
    let _ = read_cstr(0, 0);
    let _ = AF_INET;
    let _ = SOCK_DGRAM;
}
