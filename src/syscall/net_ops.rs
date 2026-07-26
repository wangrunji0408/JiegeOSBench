//! Socket syscalls.

use crate::fs::{File, OpenFlags, Result};
use crate::mm::uaccess;
use crate::net::addr::{SockAddr, AF_INET, AF_INET6, AF_NETLINK, AF_PACKET, AF_UNIX};
use crate::net::socket::{Socket, SocketKind, SOCK_CLOEXEC, SOCK_DGRAM, SOCK_NONBLOCK, SOCK_RAW, SOCK_STREAM};
use crate::{bail, task};
use alloc::sync::Arc;

/// Extract the socket behind an fd.
///
/// The returned `Arc<File>` keeps the inode — and therefore the `Socket` — alive
/// for as long as the caller holds it, so the borrow is safe across blocking
/// operations.
fn get_socket(fd: i32) -> Result<(Arc<File>, &'static Socket)> {
    let file = task::current().files.lock().get_or_err(fd)?;
    let socket = file
        .inode
        .as_any()
        .downcast_ref::<Socket>()
        .ok_or(crate::err!(ENOTSOCK))?;
    // Extend the borrow to 'static: the `Arc<File>` we return owns the inode, so
    // the referent outlives every use in this module.
    let socket: &'static Socket = unsafe { &*(socket as *const Socket) };
    Ok((file, socket))
}

pub fn sys_socket(domain: u32, ty: u32, _protocol: u32) -> Result<isize> {
    let nonblock = ty & SOCK_NONBLOCK != 0;
    let cloexec = ty & SOCK_CLOEXEC != 0;
    let base_type = ty & !(SOCK_NONBLOCK | SOCK_CLOEXEC);

    let domain16 = domain as u16;
    let kind = match (domain16, base_type) {
        (AF_INET, SOCK_STREAM) | (AF_INET6, SOCK_STREAM) => SocketKind::Tcp,
        (AF_INET, SOCK_DGRAM) | (AF_INET6, SOCK_DGRAM) => SocketKind::Udp,
        // We accept these so that probing code (getaddrinfo's netlink query,
        // nginx's unix-socket support check) gets a descriptor rather than an
        // error, but they carry no traffic.
        (AF_UNIX, _) | (AF_NETLINK, _) => SocketKind::Other,
        (AF_INET, SOCK_RAW) | (AF_PACKET, _) => bail!(EPERM),
        (AF_INET, _) | (AF_INET6, _) => bail!(EPROTONOSUPPORT),
        _ => bail!(EAFNOSUPPORT),
    };

    let socket = Socket::new(domain16, kind, nonblock);
    let mut flags = OpenFlags::RDWR;
    if nonblock {
        flags |= OpenFlags::NONBLOCK;
    }
    let file = Arc::new(File::with_path(socket, flags, "socket:[0]"));
    let fd = task::current().files.lock().insert(file, cloexec)?;
    Ok(fd as isize)
}

pub fn sys_socketpair(domain: u32, ty: u32, _protocol: u32, fds_ptr: usize) -> Result<isize> {
    // Back a socketpair with a bidirectional pipe pair: nginx doesn't use
    // socketpair on the HTTP path, but musl's `getaddrinfo` and some library
    // code create one, and pipes give correct read/write semantics.
    let _ = domain;
    let (a_read, a_write) = crate::fs::pipe::new_pipe();
    let (b_read, b_write) = crate::fs::pipe::new_pipe();

    let nonblock = ty & SOCK_NONBLOCK != 0;
    let cloexec = ty & SOCK_CLOEXEC != 0;
    let mut flags = OpenFlags::RDWR;
    if nonblock {
        flags |= OpenFlags::NONBLOCK;
    }

    // Each end reads from one pipe and writes to the other. A `File` holds a
    // single inode, so pair them up as (a_read, b_write) and (b_read, a_write)
    // using a duplex wrapper.
    let end0 = Arc::new(File::with_path(
        crate::fs::pipe::duplex(a_read, b_write),
        flags,
        "socket:[pair]",
    ));
    let end1 = Arc::new(File::with_path(
        crate::fs::pipe::duplex(b_read, a_write),
        flags,
        "socket:[pair]",
    ));

    let task = task::current();
    let fd0 = task.files.lock().insert(end0, cloexec)?;
    let fd1 = match task.files.lock().insert(end1, cloexec) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = task.files.lock().close(fd0);
            return Err(e);
        }
    };
    uaccess::write(fds_ptr, [fd0, fd1])?;
    Ok(0)
}

pub fn sys_bind(fd: i32, addr_ptr: usize, addr_len: usize) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    let addr = SockAddr::from_user(addr_ptr, addr_len)?;
    socket.bind(addr)?;
    Ok(0)
}

pub fn sys_listen(fd: i32, backlog: i32) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    let backlog = if backlog <= 0 { 1 } else { backlog as usize };
    socket.listen(backlog)?;
    Ok(0)
}

pub fn sys_accept4(fd: i32, addr_ptr: usize, len_ptr: usize, flags: u32) -> Result<isize> {
    let (file, socket) = get_socket(fd)?;
    let nonblock_request = flags & SOCK_NONBLOCK != 0;
    let cloexec = flags & SOCK_CLOEXEC != 0;

    let accepted = socket.accept(file.is_nonblock())?;
    accepted
        .nonblock
        .store(nonblock_request, core::sync::atomic::Ordering::Relaxed);

    if addr_ptr != 0 {
        if let Some(peer) = accepted.peer_addr() {
            peer.write_to_user(addr_ptr, len_ptr)?;
        }
    }

    let mut open_flags = OpenFlags::RDWR;
    if nonblock_request {
        open_flags |= OpenFlags::NONBLOCK;
    }
    let new_file = Arc::new(File::with_path(accepted, open_flags, "socket:[accepted]"));
    let new_fd = task::current().files.lock().insert(new_file, cloexec)?;
    Ok(new_fd as isize)
}

pub fn sys_connect(fd: i32, addr_ptr: usize, addr_len: usize) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    let addr = SockAddr::from_user(addr_ptr, addr_len)?;
    socket.connect(addr)?;
    Ok(0)
}

pub fn sys_getsockname(fd: i32, addr_ptr: usize, len_ptr: usize) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    let addr = socket.local_addr().unwrap_or(SockAddr::V4 {
        addr: None,
        port: 0,
    });
    addr.write_to_user(addr_ptr, len_ptr)?;
    Ok(0)
}

pub fn sys_getpeername(fd: i32, addr_ptr: usize, len_ptr: usize) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    let addr = socket.peer_addr().ok_or(crate::err!(ENOTCONN))?;
    addr.write_to_user(addr_ptr, len_ptr)?;
    Ok(0)
}

/// `send`/`recv` flags.
const MSG_OOB: u32 = 1;
const MSG_PEEK: u32 = 2;
const MSG_DONTROUTE: u32 = 4;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_NOSIGNAL: u32 = 0x4000;

pub fn sys_sendto(
    fd: i32,
    buf: usize,
    len: usize,
    flags: u32,
    addr_ptr: usize,
    addr_len: usize,
) -> Result<isize> {
    let (file, socket) = get_socket(fd)?;
    if len == 0 {
        return Ok(0);
    }
    let data = uaccess::read_bytes(buf, len.min(16 * 1024 * 1024))?;
    let nonblock = flags & MSG_DONTWAIT != 0 || file.is_nonblock();

    let n = if addr_ptr != 0 && addr_len > 0 {
        let addr = SockAddr::from_user(addr_ptr, addr_len)?;
        socket.send_to(&data, &addr, nonblock)?
    } else {
        socket.send(&data, nonblock)?
    };
    Ok(n as isize)
}

pub fn sys_recvfrom(
    fd: i32,
    buf: usize,
    len: usize,
    flags: u32,
    addr_ptr: usize,
    len_ptr: usize,
) -> Result<isize> {
    let (file, socket) = get_socket(fd)?;
    if len == 0 {
        return Ok(0);
    }
    let mut data = alloc::vec![0u8; len.min(16 * 1024 * 1024)];
    let nonblock = flags & MSG_DONTWAIT != 0 || file.is_nonblock();
    let peek = flags & MSG_PEEK != 0;

    let (n, sender) = if socket.kind == SocketKind::Udp {
        socket.recv_udp(&mut data, nonblock)?
    } else {
        (socket.recv(&mut data, nonblock, peek)?, None)
    };

    uaccess::write_bytes(buf, &data[..n])?;
    if addr_ptr != 0 {
        let addr = sender
            .or_else(|| socket.peer_addr())
            .unwrap_or(SockAddr::V4 {
                addr: None,
                port: 0,
            });
        addr.write_to_user(addr_ptr, len_ptr)?;
    }
    Ok(n as isize)
}

/// `struct msghdr`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MsgHdr {
    name: usize,
    namelen: u32,
    _pad1: u32,
    iov: usize,
    iovlen: usize,
    control: usize,
    controllen: usize,
    flags: i32,
    _pad2: i32,
}

pub fn sys_sendmsg(fd: i32, msg_ptr: usize, flags: u32) -> Result<isize> {
    let (file, socket) = get_socket(fd)?;
    let msg: MsgHdr = uaccess::read(msg_ptr)?;
    let vecs = uaccess::read_iovecs(msg.iov, msg.iovlen)?;
    let nonblock = flags & MSG_DONTWAIT != 0 || file.is_nonblock();

    // Gather the iovecs into one buffer so a datagram goes out as one packet.
    let mut data = alloc::vec::Vec::new();
    for v in &vecs {
        if v.len == 0 {
            continue;
        }
        data.extend_from_slice(&uaccess::read_bytes(v.base, v.len.min(1024 * 1024))?);
    }
    if data.is_empty() {
        return Ok(0);
    }

    let n = if msg.name != 0 && msg.namelen > 0 {
        let addr = SockAddr::from_user(msg.name, msg.namelen as usize)?;
        socket.send_to(&data, &addr, nonblock)?
    } else {
        socket.send(&data, nonblock)?
    };
    Ok(n as isize)
}

pub fn sys_recvmsg(fd: i32, msg_ptr: usize, flags: u32) -> Result<isize> {
    let (file, socket) = get_socket(fd)?;
    let mut msg: MsgHdr = uaccess::read(msg_ptr)?;
    let vecs = uaccess::read_iovecs(msg.iov, msg.iovlen)?;
    let nonblock = flags & MSG_DONTWAIT != 0 || file.is_nonblock();

    let total: usize = vecs.iter().map(|v| v.len).sum();
    if total == 0 {
        return Ok(0);
    }
    let mut data = alloc::vec![0u8; total.min(16 * 1024 * 1024)];
    let (n, sender) = if socket.kind == SocketKind::Udp {
        socket.recv_udp(&mut data, nonblock)?
    } else {
        (
            socket.recv(&mut data, nonblock, flags & MSG_PEEK != 0)?,
            None,
        )
    };

    // Scatter the data back into the iovecs.
    let mut written = 0;
    for v in &vecs {
        if written >= n {
            break;
        }
        let chunk = v.len.min(n - written);
        uaccess::write_bytes(v.base, &data[written..written + chunk])?;
        written += chunk;
    }

    if msg.name != 0 {
        if let Some(addr) = sender.or_else(|| socket.peer_addr()) {
            let (bytes, full_len) = addr.to_bytes();
            let copy = (msg.namelen as usize).min(bytes.len());
            uaccess::write_bytes(msg.name, &bytes[..copy])?;
            msg.namelen = full_len as u32;
        } else {
            msg.namelen = 0;
        }
    }
    msg.controllen = 0;
    msg.flags = 0;
    uaccess::write(msg_ptr, msg)?;
    Ok(n as isize)
}

pub fn sys_shutdown(fd: i32, how: i32) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    socket.shutdown(how)?;
    Ok(0)
}

// Socket option levels and names.
const SOL_SOCKET: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_IP: i32 = 0;
const IPPROTO_IPV6: i32 = 41;

const SO_DEBUG: i32 = 1;
const SO_REUSEADDR: i32 = 2;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_DONTROUTE: i32 = 5;
const SO_BROADCAST: i32 = 6;
const SO_SNDBUF: i32 = 7;
const SO_RCVBUF: i32 = 8;
const SO_KEEPALIVE: i32 = 9;
const SO_LINGER: i32 = 13;
const SO_REUSEPORT: i32 = 15;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;
const SO_ACCEPTCONN: i32 = 30;
const SO_PROTOCOL: i32 = 38;
const SO_DOMAIN: i32 = 39;

const TCP_NODELAY: i32 = 1;
const TCP_KEEPIDLE: i32 = 4;
const TCP_KEEPINTVL: i32 = 5;
const TCP_KEEPCNT: i32 = 6;
const TCP_FASTOPEN: i32 = 23;
const TCP_DEFER_ACCEPT: i32 = 9;
const TCP_CORK: i32 = 3;
const TCP_INFO: i32 = 11;

const IPV6_V6ONLY: i32 = 26;

pub fn sys_setsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value_ptr: usize,
    value_len: u32,
) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    use core::sync::atomic::Ordering;

    // Most options are a single int.
    let int_value = if value_len >= 4 && value_ptr != 0 {
        uaccess::read::<i32>(value_ptr)?
    } else {
        0
    };

    match (level, name) {
        (SOL_SOCKET, SO_REUSEADDR) => {
            socket.reuseaddr.store(int_value != 0, Ordering::Relaxed);
            Ok(0)
        }
        (SOL_SOCKET, SO_REUSEPORT) => {
            socket.reuseport.store(int_value != 0, Ordering::Relaxed);
            Ok(0)
        }
        (SOL_SOCKET, SO_KEEPALIVE) => {
            socket.keepalive.store(int_value != 0, Ordering::Relaxed);
            Ok(0)
        }
        (SOL_SOCKET, SO_SNDBUF) => {
            // Linux doubles the requested value; report it back the same way.
            socket
                .sndbuf
                .store((int_value as usize).clamp(4096, 4 * 1024 * 1024), Ordering::Relaxed);
            Ok(0)
        }
        (SOL_SOCKET, SO_RCVBUF) => {
            socket
                .rcvbuf
                .store((int_value as usize).clamp(4096, 4 * 1024 * 1024), Ordering::Relaxed);
            Ok(0)
        }
        (SOL_SOCKET, SO_RCVTIMEO) | (SOL_SOCKET, SO_SNDTIMEO) => {
            let tv: crate::fs::stat::Timeval = uaccess::read(value_ptr)?;
            let ms = (tv.sec as u64 * 1000 + tv.usec as u64 / 1000) as u32;
            if name == SO_RCVTIMEO {
                socket.rcvtimeo_ms.store(ms, Ordering::Relaxed);
            } else {
                socket.sndtimeo_ms.store(ms, Ordering::Relaxed);
            }
            Ok(0)
        }
        (IPPROTO_TCP, TCP_NODELAY) => {
            // Push it through to smoltcp, not just into our record. nginx sets
            // this on every accepted connection; leaving Nagle enabled makes
            // smoltcp withhold the response segment while an earlier one is
            // unacknowledged, so the client waits out a retransmit timer instead
            // of getting its reply.
            socket.set_nodelay(int_value != 0);
            Ok(0)
        }
        // Accepted and ignored: these tune behaviour we don't implement, and
        // failing them would make nginx abort at startup.
        (SOL_SOCKET, SO_LINGER)
        | (SOL_SOCKET, SO_BROADCAST)
        | (SOL_SOCKET, SO_DONTROUTE)
        | (SOL_SOCKET, SO_DEBUG)
        | (IPPROTO_TCP, TCP_KEEPIDLE)
        | (IPPROTO_TCP, TCP_KEEPINTVL)
        | (IPPROTO_TCP, TCP_KEEPCNT)
        | (IPPROTO_TCP, TCP_FASTOPEN)
        | (IPPROTO_TCP, TCP_DEFER_ACCEPT)
        | (IPPROTO_TCP, TCP_CORK)
        | (IPPROTO_IPV6, IPV6_V6ONLY) => Ok(0),
        _ => {
            crate::trace!("setsockopt: ignoring level {} option {}", level, name);
            Ok(0)
        }
    }
}

pub fn sys_getsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value_ptr: usize,
    len_ptr: usize,
) -> Result<isize> {
    let (_, socket) = get_socket(fd)?;
    use core::sync::atomic::Ordering;

    /// Write an int result and set the length.
    fn write_int(value_ptr: usize, len_ptr: usize, value: i32) -> Result<isize> {
        if value_ptr != 0 {
            uaccess::write(value_ptr, value)?;
        }
        if len_ptr != 0 {
            uaccess::write(len_ptr, 4u32)?;
        }
        Ok(0)
    }

    match (level, name) {
        (SOL_SOCKET, SO_ERROR) => {
            // Reading SO_ERROR clears it, which is how non-blocking `connect`
            // failures are collected.
            let error = socket.error.swap(0, Ordering::Relaxed);
            write_int(value_ptr, len_ptr, error)
        }
        (SOL_SOCKET, SO_TYPE) => write_int(
            value_ptr,
            len_ptr,
            match socket.kind {
                SocketKind::Tcp => SOCK_STREAM as i32,
                SocketKind::Udp => SOCK_DGRAM as i32,
                SocketKind::Other => SOCK_STREAM as i32,
            },
        ),
        (SOL_SOCKET, SO_DOMAIN) => write_int(value_ptr, len_ptr, socket.family as i32),
        (SOL_SOCKET, SO_PROTOCOL) => write_int(
            value_ptr,
            len_ptr,
            match socket.kind {
                SocketKind::Tcp => IPPROTO_TCP,
                _ => 0,
            },
        ),
        (SOL_SOCKET, SO_REUSEADDR) => {
            write_int(value_ptr, len_ptr, socket.reuseaddr.load(Ordering::Relaxed) as i32)
        }
        (SOL_SOCKET, SO_REUSEPORT) => {
            write_int(value_ptr, len_ptr, socket.reuseport.load(Ordering::Relaxed) as i32)
        }
        (SOL_SOCKET, SO_KEEPALIVE) => {
            write_int(value_ptr, len_ptr, socket.keepalive.load(Ordering::Relaxed) as i32)
        }
        (SOL_SOCKET, SO_SNDBUF) => write_int(
            value_ptr,
            len_ptr,
            socket.sndbuf.load(Ordering::Relaxed) as i32,
        ),
        (SOL_SOCKET, SO_RCVBUF) => write_int(
            value_ptr,
            len_ptr,
            socket.rcvbuf.load(Ordering::Relaxed) as i32,
        ),
        (SOL_SOCKET, SO_ACCEPTCONN) => write_int(
            value_ptr,
            len_ptr,
            (socket.state() == crate::net::socket::SockState::Listening) as i32,
        ),
        (IPPROTO_TCP, TCP_NODELAY) => {
            write_int(value_ptr, len_ptr, socket.nodelay.load(Ordering::Relaxed) as i32)
        }
        (IPPROTO_TCP, TCP_INFO) => {
            // Report a zeroed `tcp_info`; nothing we run inspects the fields.
            if len_ptr != 0 {
                let want: u32 = uaccess::read(len_ptr)?;
                let n = (want as usize).min(104);
                uaccess::write_bytes(value_ptr, &alloc::vec![0u8; n])?;
                uaccess::write(len_ptr, n as u32)?;
            }
            Ok(0)
        }
        _ => write_int(value_ptr, len_ptr, 0),
    }
}

/// Keep the remaining flag constants documented.
const _: u32 = MSG_OOB | MSG_DONTROUTE | MSG_NOSIGNAL;
const _: i32 = IPPROTO_IP;
