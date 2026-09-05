//! Socket system calls.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::fs::{get_file, install_fd};
use crate::abi::*;
use crate::fs::file::{File, FileOps};
use crate::mm::uaccess::*;
use crate::net::socket::{Ancillary, SockAddr, SocketOps};
use crate::task::current;

fn sock_of(file: &File) -> Result<&dyn SocketOps, i32> {
    file.ops.as_socket().ok_or(ENOTSOCK)
}

fn read_sockaddr(addr: usize, len: u32) -> Result<SockAddr, i32> {
    if addr == 0 || len < 2 {
        return Err(EINVAL);
    }
    let bytes = read_bytes(addr, (len as usize).min(128))?;
    SockAddr::parse(&bytes)
}

fn write_sockaddr(sa: &SockAddr, addr: usize, lenp: usize) -> Result<(), i32> {
    if addr == 0 || lenp == 0 {
        return Ok(());
    }
    let cap: u32 = read_val(lenp)?;
    let bytes = sa.to_bytes();
    let n = bytes.len().min(cap as usize);
    copy_to_user(addr, &bytes[..n])?;
    write_val(lenp, bytes.len() as u32)?;
    Ok(())
}

pub fn sys_socket(domain: u32, ty: u32, protocol: u32) -> SysResult {
    let kind = ty & SOCK_TYPE_MASK;
    let ops: Arc<dyn FileOps> = match (domain as u16, kind) {
        (AF_INET, SOCK_STREAM) => {
            if protocol != 0 && protocol != 6 {
                return Err(EPROTONOSUPPORT);
            }
            if !crate::net::is_up() {
                return Err(EAFNOSUPPORT);
            }
            crate::net::tcp::TcpSocket::new()
        }
        (AF_INET, SOCK_DGRAM) => {
            if protocol != 0 && protocol != 17 {
                return Err(EPROTONOSUPPORT);
            }
            if !crate::net::is_up() {
                return Err(EAFNOSUPPORT);
            }
            crate::net::udp::UdpSocket::new()
        }
        (AF_UNIX, SOCK_STREAM) | (AF_UNIX, SOCK_DGRAM) => {
            // Unconnected unix sockets are not supported (no filesystem namespace);
            // create a dangling endpoint so that socket() succeeds.
            let (a, _b) = crate::net::unix::socketpair(kind == SOCK_STREAM);
            a
        }
        (AF_INET6, _) => return Err(EAFNOSUPPORT),
        (AF_NETLINK, _) => return Err(EAFNOSUPPORT),
        _ => return Err(EAFNOSUPPORT),
    };
    let flags = O_RDWR | (ty & SOCK_NONBLOCK);
    let f = File::new(ops, flags, String::from("socket:"));
    install_fd(f, ty & SOCK_CLOEXEC != 0)
}

pub fn sys_socketpair(domain: u32, ty: u32, _protocol: u32, sv: usize) -> SysResult {
    if domain as u16 != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let kind = ty & SOCK_TYPE_MASK;
    if kind != SOCK_STREAM && kind != SOCK_DGRAM && kind != 5 {
        return Err(EPROTONOSUPPORT);
    }
    let (a, b) = crate::net::unix::socketpair(kind == SOCK_STREAM);
    let flags = O_RDWR | (ty & SOCK_NONBLOCK);
    let fa = File::new(a, flags, String::from("socket:"));
    let fb = File::new(b, flags, String::from("socket:"));
    let cloexec = ty & SOCK_CLOEXEC != 0;
    let fds = current().fds();
    let mut t = fds.lock();
    let fd0 = t.alloc(fa, cloexec, 0)?;
    let fd1 = match t.alloc(fb, cloexec, 0) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = t.close(fd0);
            return Err(e);
        }
    };
    drop(t);
    write_val(sv, [fd0, fd1])?;
    Ok(0)
}

pub fn sys_bind(fd: i32, addr: usize, len: u32) -> SysResult {
    let file = get_file(fd)?;
    let sa = read_sockaddr(addr, len)?;
    sock_of(&file)?.bind(sa)?;
    Ok(0)
}

pub fn sys_listen(fd: i32, backlog: i32) -> SysResult {
    let file = get_file(fd)?;
    sock_of(&file)?.listen(backlog)?;
    Ok(0)
}

pub fn sys_accept4(fd: i32, addr: usize, lenp: usize, flags: u32) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let (ops, peer) = sock.accept(file.nonblock())?;
    let nf = File::new(ops, O_RDWR | (flags & SOCK_NONBLOCK), String::from("socket:"));
    let nfd = install_fd(nf, flags & SOCK_CLOEXEC != 0)?;
    write_sockaddr(&peer, addr, lenp)?;
    Ok(nfd)
}

pub fn sys_connect(fd: i32, addr: usize, len: u32) -> SysResult {
    let file = get_file(fd)?;
    let sa = read_sockaddr(addr, len)?;
    sock_of(&file)?.connect(sa, file.nonblock())?;
    Ok(0)
}

pub fn sys_getsockname(fd: i32, addr: usize, lenp: usize) -> SysResult {
    let file = get_file(fd)?;
    let sa = sock_of(&file)?.local_addr()?;
    write_sockaddr(&sa, addr, lenp)?;
    Ok(0)
}

pub fn sys_getpeername(fd: i32, addr: usize, lenp: usize) -> SysResult {
    let file = get_file(fd)?;
    let sa = sock_of(&file)?.peer_addr()?;
    write_sockaddr(&sa, addr, lenp)?;
    Ok(0)
}

pub fn sys_sendto(fd: i32, buf: usize, len: usize, flags: u32, addr: usize, addrlen: u32) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let data = read_bytes(buf, len.min(1024 * 1024))?;
    let to = if addr != 0 && addrlen > 0 { Some(read_sockaddr(addr, addrlen)?) } else { None };
    sock.send(&data, flags, file.nonblock(), to, Ancillary::default())
}

pub fn sys_recvfrom(fd: i32, buf: usize, len: usize, flags: u32, addr: usize, lenp: usize) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let mut kbuf = alloc::vec![0u8; len.min(1024 * 1024)];
    let (n, from, _anc) = sock.recv(&mut kbuf, flags, file.nonblock())?;
    copy_to_user(buf, &kbuf[..n])?;
    if let Some(sa) = from {
        write_sockaddr(&sa, addr, lenp)?;
    }
    Ok(n)
}

pub fn sys_setsockopt(fd: i32, level: i32, opt: i32, val: usize, len: u32) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let v = if val != 0 { read_bytes(val, (len as usize).min(256))? } else { Vec::new() };
    sock.setsockopt(level, opt, &v)?;
    Ok(0)
}

pub fn sys_getsockopt(fd: i32, level: i32, opt: i32, val: usize, lenp: usize) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let v = sock.getsockopt(level, opt)?;
    let cap: u32 = read_val(lenp)?;
    let n = v.len().min(cap as usize);
    copy_to_user(val, &v[..n])?;
    write_val(lenp, n as u32)?;
    Ok(0)
}

pub fn sys_shutdown(fd: i32, how: i32) -> SysResult {
    let file = get_file(fd)?;
    sock_of(&file)?.shutdown(how)?;
    Ok(0)
}

fn read_iovs(iov: usize, cnt: usize) -> Result<Vec<Iovec>, i32> {
    if cnt > 1024 {
        return Err(EMSGSIZE);
    }
    let mut v = Vec::with_capacity(cnt);
    for i in 0..cnt {
        v.push(read_val::<Iovec>(iov + i * 16)?);
    }
    Ok(v)
}

pub fn sys_sendmsg(fd: i32, msg: usize, flags: u32) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let hdr: MsgHdr = read_val(msg)?;
    let iovs = read_iovs(hdr.msg_iov, hdr.msg_iovlen)?;
    let mut data = Vec::new();
    for v in &iovs {
        let start = data.len();
        data.resize(start + v.len, 0);
        copy_from_user(&mut data[start..], v.base)?;
    }
    let to = if hdr.msg_name != 0 && hdr.msg_namelen > 0 { Some(read_sockaddr(hdr.msg_name, hdr.msg_namelen)?) } else { None };
    // Parse control messages (SCM_RIGHTS).
    let mut anc = Ancillary::default();
    if hdr.msg_control != 0 && hdr.msg_controllen >= 16 {
        let ctl = read_bytes(hdr.msg_control, hdr.msg_controllen.min(4096))?;
        let mut off = 0;
        while off + 16 <= ctl.len() {
            let clen = usize::from_le_bytes(ctl[off..off + 8].try_into().unwrap());
            let level = i32::from_le_bytes(ctl[off + 8..off + 12].try_into().unwrap());
            let ty = i32::from_le_bytes(ctl[off + 12..off + 16].try_into().unwrap());
            if clen < 16 || off + clen > ctl.len() {
                break;
            }
            if level == SOL_SOCKET && ty == SCM_RIGHTS {
                let fds_bytes = &ctl[off + 16..off + clen];
                let table = current().fds();
                let t = table.lock();
                for chunk in fds_bytes.chunks_exact(4) {
                    let sfd = i32::from_le_bytes(chunk.try_into().unwrap());
                    anc.fds.push(t.get(sfd)?);
                }
            }
            off += (clen + 7) & !7;
        }
    }
    sock.send(&data, flags, file.nonblock(), to, anc)
}

pub fn sys_recvmsg(fd: i32, msg: usize, flags: u32) -> SysResult {
    let file = get_file(fd)?;
    let sock = sock_of(&file)?;
    let mut hdr: MsgHdr = read_val(msg)?;
    let iovs = read_iovs(hdr.msg_iov, hdr.msg_iovlen)?;
    let total: usize = iovs.iter().map(|v| v.len).sum::<usize>().min(1024 * 1024);
    let mut kbuf = alloc::vec![0u8; total];
    let (n, from, anc) = sock.recv(&mut kbuf, flags, file.nonblock())?;
    let mut done = 0;
    for v in &iovs {
        if done >= n {
            break;
        }
        let take = v.len.min(n - done);
        copy_to_user(v.base, &kbuf[done..done + take])?;
        done += take;
    }
    if let Some(sa) = from {
        if hdr.msg_name != 0 {
            let bytes = sa.to_bytes();
            let m = bytes.len().min(hdr.msg_namelen as usize);
            copy_to_user(hdr.msg_name, &bytes[..m])?;
            hdr.msg_namelen = bytes.len() as u32;
        }
    } else {
        hdr.msg_namelen = 0;
    }
    hdr.msg_flags = 0;
    // Deliver passed fds.
    if !anc.fds.is_empty() && hdr.msg_control != 0 {
        let need = 16 + anc.fds.len() * 4;
        if hdr.msg_controllen >= need {
            let mut ctl = Vec::with_capacity((need + 7) & !7);
            ctl.extend_from_slice(&(need as u64).to_le_bytes());
            ctl.extend_from_slice(&SOL_SOCKET.to_le_bytes());
            ctl.extend_from_slice(&SCM_RIGHTS.to_le_bytes());
            let table = current().fds();
            for f in anc.fds {
                let nfd = table.lock().alloc(f, flags & MSG_CMSG_CLOEXEC != 0, 0)?;
                ctl.extend_from_slice(&nfd.to_le_bytes());
            }
            while ctl.len() % 8 != 0 {
                ctl.push(0);
            }
            copy_to_user(hdr.msg_control, &ctl)?;
            hdr.msg_controllen = ctl.len();
        } else {
            hdr.msg_flags |= MSG_CTRUNC as i32;
            hdr.msg_controllen = 0;
        }
    } else {
        hdr.msg_controllen = 0;
    }
    write_val(msg, hdr)?;
    Ok(n)
}
