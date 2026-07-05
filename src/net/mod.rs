//! Network stack: virtio-net + smoltcp, kernel socket table, epoll.
pub mod virtio;

use crate::syscall::{
    check_user_range, read_user, user_slice, user_slice_mut, write_user, SysResult, EAGAIN, EBADF,
    ECONNRESET, EINVAL, ENOTCONN, ENOTSOCK, EOPNOTSUPP,
};
use crate::task::current;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

const LISTEN_POOL: usize = 16;
const SOCK_BUF: usize = 64 * 1024;

pub struct NetDev(pub virtio::VirtioNet);

pub struct VRxToken(Vec<u8>);
pub struct VTxToken<'a>(&'a mut virtio::VirtioNet);

impl RxToken for VRxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl<'a> TxToken for VTxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = alloc::vec![0u8; len];
        let r = f(&mut buf);
        self.0.send(&buf);
        r
    }
}

impl Device for NetDev {
    type RxToken<'a> = VRxToken;
    type TxToken<'a> = VTxToken<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(VRxToken, VTxToken<'_>)> {
        let (id, frame) = self.0.recv()?;
        let data = frame.to_vec();
        self.0.recycle_rx(id);
        // Safety: splitting borrow — recv/recycle done, now hand out tx side
        let dev = unsafe { &mut *(&mut self.0 as *mut virtio::VirtioNet) };
        Some((VRxToken(data), VTxToken(dev)))
    }
    fn transmit(&mut self, _ts: Instant) -> Option<VTxToken<'_>> {
        Some(VTxToken(&mut self.0))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.medium = Medium::Ethernet;
        caps
    }
}

enum Kind {
    TcpNew { port: Option<u16> },
    TcpListen { port: u16, pool: Vec<SocketHandle> },
    TcpStream { h: SocketHandle, port: u16 },
}

struct KSock {
    refs: usize,
    kind: Kind,
}

pub struct Epoll {
    // fd -> (events, data)
    pub interest: BTreeMap<usize, (u32, u64)>,
}

pub struct NetStack {
    dev: NetDev,
    iface: Interface,
    sockets: SocketSet<'static>,
    ksocks: Vec<Option<KSock>>,
    pub epolls: Vec<Option<Epoll>>,
    closing: Vec<SocketHandle>,
}

static mut NET: Option<NetStack> = None;

#[allow(static_mut_refs)]
fn net() -> &'static mut NetStack {
    unsafe { NET.as_mut().expect("net not initialized") }
}

fn now() -> Instant {
    Instant::from_micros((crate::time::now_ns() / 1000) as i64)
}

pub fn init() {
    let vdev = virtio::probe().expect("no virtio-net device found");
    let mac = vdev.mac;
    let mut dev = NetDev(vdev);
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let mut iface = Interface::new(config, &mut dev, now());
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
            .unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .unwrap();
    println!("[net] ip 10.0.2.15/24 gw 10.0.2.2");
    unsafe {
        NET = Some(NetStack {
            dev,
            iface,
            sockets: SocketSet::new(Vec::new()),
            ksocks: Vec::new(),
            epolls: Vec::new(),
            closing: Vec::new(),
        });
    }
}

fn new_tcp_socket() -> tcp::Socket<'static> {
    let rx = tcp::SocketBuffer::new(alloc::vec![0u8; SOCK_BUF]);
    let tx = tcp::SocketBuffer::new(alloc::vec![0u8; SOCK_BUF]);
    tcp::Socket::new(rx, tx)
}

/// Pump the network stack: process rx/tx, replenish listener pools,
/// garbage-collect closed sockets.
pub fn poll() {
    let n = net();
    n.iface.poll(now(), &mut n.dev, &mut n.sockets);

    // replenish listener pools
    for ks in n.ksocks.iter_mut().flatten() {
        if let Kind::TcpListen { port, pool } = &mut ks.kind {
            let listening = pool
                .iter()
                .filter(|h| n.sockets.get::<tcp::Socket>(**h).state() == tcp::State::Listen)
                .count();
            if listening == 0 && pool.len() < LISTEN_POOL * 2 {
                for _ in 0..4 {
                    let mut s = new_tcp_socket();
                    s.listen(*port).ok();
                    pool.push(n.sockets.add(s));
                }
            }
        }
    }

    // gc closing sockets
    let mut i = 0;
    while i < n.closing.len() {
        let h = n.closing[i];
        let done = {
            let s = n.sockets.get_mut::<tcp::Socket>(h);
            s.state() == tcp::State::Closed
        };
        if done {
            n.sockets.remove(h);
            n.closing.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

fn sock_id(fd: usize) -> Result<usize, i32> {
    match current().fds.get(fd).map(|e| &e.obj) {
        Some(crate::fs::FdObj::Socket(id)) => Ok(*id),
        Some(_) => Err(ENOTSOCK),
        None => Err(EBADF),
    }
}

fn alloc_ksock(k: KSock) -> usize {
    let n = net();
    for (i, s) in n.ksocks.iter_mut().enumerate() {
        if s.is_none() {
            *s = Some(k);
            return i;
        }
    }
    n.ksocks.push(Some(k));
    n.ksocks.len() - 1
}

pub fn socket_dup(id: usize) {
    if let Some(ks) = net().ksocks[id].as_mut() {
        ks.refs += 1;
    }
}

pub fn socket_close(id: usize) {
    let n = net();
    let Some(ks) = n.ksocks[id].as_mut() else {
        return;
    };
    ks.refs -= 1;
    if ks.refs > 0 {
        return;
    }
    let ks = n.ksocks[id].take().unwrap();
    match ks.kind {
        Kind::TcpStream { h, .. } => {
            n.sockets.get_mut::<tcp::Socket>(h).close();
            n.closing.push(h);
        }
        Kind::TcpListen { pool, .. } => {
            for h in pool {
                n.sockets.get_mut::<tcp::Socket>(h).abort();
                n.closing.push(h);
            }
        }
        Kind::TcpNew { .. } => {}
    }
    poll();
}

// ---------- syscalls ----------

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOCK_NONBLOCK: usize = 0x800;
const SOCK_CLOEXEC: usize = 0x80000;

pub fn socket(domain: usize, ty: usize, _proto: usize) -> SysResult {
    if domain != AF_INET {
        return Err(97); // EAFNOSUPPORT
    }
    if ty & 0xff != SOCK_STREAM {
        return Err(EINVAL);
    }
    let id = alloc_ksock(KSock {
        refs: 1,
        kind: Kind::TcpNew { port: None },
    });
    let fd = current().fds.alloc(crate::fs::FdEntry {
        obj: crate::fs::FdObj::Socket(id),
        cloexec: ty & SOCK_CLOEXEC != 0,
        nonblock: ty & SOCK_NONBLOCK != 0,
    });
    Ok(fd)
}

pub fn socketpair(_d: usize, _t: usize, _p: usize, _sv: usize) -> SysResult {
    Err(EOPNOTSUPP)
}

fn parse_sockaddr_in(addr: usize) -> Result<(u32, u16), i32> {
    let family: u16 = read_user(addr)?;
    if family as usize != AF_INET {
        return Err(EINVAL);
    }
    let port_be: u16 = read_user(addr + 2)?;
    let ip_be: u32 = read_user(addr + 4)?;
    Ok((u32::from_be(ip_be), u16::from_be(port_be)))
}

fn write_sockaddr_in(addr: usize, len_ptr: usize, ip: [u8; 4], port: u16) -> Result<(), i32> {
    if addr == 0 {
        return Ok(());
    }
    check_user_range(addr, 16)?;
    write_user(addr, AF_INET as u16)?;
    write_user(addr + 2, port.to_be())?;
    let dst = user_slice_mut(addr + 4, 4)?;
    dst.copy_from_slice(&ip);
    unsafe { core::ptr::write_bytes((addr + 8) as *mut u8, 0, 8) };
    if len_ptr != 0 {
        write_user(len_ptr, 16u32)?;
    }
    Ok(())
}

pub fn bind(fd: usize, addr: usize, _len: usize) -> SysResult {
    let id = sock_id(fd)?;
    let (_ip, port) = parse_sockaddr_in(addr)?;
    let n = net();
    match &mut n.ksocks[id].as_mut().ok_or(EBADF)?.kind {
        Kind::TcpNew { port: p } => {
            *p = Some(port);
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub fn listen(fd: usize, _backlog: usize) -> SysResult {
    let id = sock_id(fd)?;
    let n = net();
    let ks = n.ksocks[id].as_mut().ok_or(EBADF)?;
    let port = match &ks.kind {
        Kind::TcpNew { port: Some(p) } => *p,
        Kind::TcpListen { .. } => return Ok(0),
        _ => return Err(EINVAL),
    };
    let mut pool = Vec::new();
    for _ in 0..LISTEN_POOL {
        let mut s = new_tcp_socket();
        s.listen(port).map_err(|_| 98)?; // EADDRINUSE
        pool.push(n.sockets.add(s));
    }
    ks.kind = Kind::TcpListen { port, pool };
    println!("[net] listening on port {}", port);
    Ok(0)
}

/// Find a connection in the pool that has progressed past Listen and is usable.
fn pool_take_ready(n: &mut NetStack, id: usize) -> Option<SocketHandle> {
    let Some(KSock {
        kind: Kind::TcpListen { pool, .. },
        ..
    }) = n.ksocks[id].as_mut()
    else {
        return None;
    };
    for (i, h) in pool.iter().enumerate() {
        let s = n.sockets.get::<tcp::Socket>(*h);
        match s.state() {
            tcp::State::Established | tcp::State::CloseWait => {
                let h = *h;
                pool.swap_remove(i);
                return Some(h);
            }
            _ => {}
        }
    }
    None
}

fn pool_has_ready(n: &mut NetStack, id: usize) -> bool {
    let Some(KSock {
        kind: Kind::TcpListen { pool, .. },
        ..
    }) = n.ksocks[id].as_ref()
    else {
        return false;
    };
    pool.iter().any(|h| {
        matches!(
            n.sockets.get::<tcp::Socket>(*h).state(),
            tcp::State::Established | tcp::State::CloseWait
        )
    })
}

pub fn accept4(fd: usize, addr: usize, addrlen: usize, flags: usize) -> SysResult {
    let id = sock_id(fd)?;
    let nonblock = current().fds.get(fd).map(|e| e.nonblock).unwrap_or(false);
    loop {
        poll();
        let n = net();
        if let Some(h) = pool_take_ready(n, id) {
            // replenish pool
            if let Some(KSock {
                kind: Kind::TcpListen { port, pool },
                ..
            }) = n.ksocks[id].as_mut()
            {
                let mut s = new_tcp_socket();
                s.listen(*port).ok();
                pool.push(n.sockets.add(s));
            }
            let port = match &n.ksocks[id].as_ref().unwrap().kind {
                Kind::TcpListen { port, .. } => *port,
                _ => 0,
            };
            let (peer_ip, peer_port) = {
                let s = n.sockets.get::<tcp::Socket>(h);
                match s.remote_endpoint() {
                    Some(ep) => {
                        let ip = match ep.addr {
                            IpAddress::Ipv4(v4) => v4.octets(),
                        };
                        (ip, ep.port)
                    }
                    None => ([0, 0, 0, 0], 0),
                }
            };
            let nid = alloc_ksock(KSock {
                refs: 1,
                kind: Kind::TcpStream { h, port },
            });
            let newfd = current().fds.alloc(crate::fs::FdEntry {
                obj: crate::fs::FdObj::Socket(nid),
                cloexec: flags & SOCK_CLOEXEC != 0,
                nonblock: flags & SOCK_NONBLOCK != 0,
            });
            write_sockaddr_in(addr, addrlen, peer_ip, peer_port)?;
            return Ok(newfd);
        }
        if nonblock {
            return Err(EAGAIN);
        }
        if current().exit_code.is_some() {
            return Err(EINVAL);
        }
    }
}

pub fn connect(_fd: usize, _addr: usize, _len: usize) -> SysResult {
    Err(101) // ENETUNREACH — outbound not needed for a server
}

pub fn getsockname(fd: usize, addr: usize, len: usize) -> SysResult {
    let id = sock_id(fd)?;
    let n = net();
    let port = match &n.ksocks[id].as_ref().ok_or(EBADF)?.kind {
        Kind::TcpNew { port } => port.unwrap_or(0),
        Kind::TcpListen { port, .. } => *port,
        Kind::TcpStream { port, .. } => *port,
    };
    write_sockaddr_in(addr, len, [10, 0, 2, 15], port)?;
    Ok(0)
}

pub fn getpeername(fd: usize, addr: usize, len: usize) -> SysResult {
    let id = sock_id(fd)?;
    let n = net();
    match &n.ksocks[id].as_ref().ok_or(EBADF)?.kind {
        Kind::TcpStream { h, .. } => {
            let s = n.sockets.get::<tcp::Socket>(*h);
            match s.remote_endpoint() {
                Some(ep) => {
                    let ip = match ep.addr {
                        IpAddress::Ipv4(v4) => v4.octets(),
                    };
                    write_sockaddr_in(addr, len, ip, ep.port)?;
                    Ok(0)
                }
                None => Err(ENOTCONN),
            }
        }
        _ => Err(ENOTCONN),
    }
}

fn stream_handle(id: usize) -> Result<SocketHandle, i32> {
    match &net().ksocks[id].as_ref().ok_or(EBADF)?.kind {
        Kind::TcpStream { h, .. } => Ok(*h),
        _ => Err(ENOTCONN),
    }
}

pub fn socket_recv_available(id: usize) -> usize {
    match stream_handle(id) {
        Ok(h) => net().sockets.get::<tcp::Socket>(h).recv_queue(),
        Err(_) => 0,
    }
}

const MSG_PEEK: usize = 2;
const MSG_DONTWAIT: usize = 0x40;

pub fn socket_recv(id: usize, buf: usize, len: usize, nonblock: bool) -> SysResult {
    socket_recv_flags(id, buf, len, if nonblock { MSG_DONTWAIT } else { 0 })
}

pub fn socket_recv_flags(id: usize, buf: usize, len: usize, flags: usize) -> SysResult {
    let h = stream_handle(id)?;
    let dst = user_slice_mut(buf, len)?;
    loop {
        poll();
        let n = net();
        let s = n.sockets.get_mut::<tcp::Socket>(h);
        if s.can_recv() {
            let r = if flags & MSG_PEEK != 0 {
                s.peek_slice(dst)
            } else {
                s.recv_slice(dst)
            };
            return r.map_err(|_| ECONNRESET);
        }
        match s.state() {
            tcp::State::Closed | tcp::State::CloseWait | tcp::State::Closing
            | tcp::State::TimeWait | tcp::State::LastAck => return Ok(0), // EOF
            _ => {}
        }
        if !s.is_active() {
            return Ok(0);
        }
        if flags & MSG_DONTWAIT != 0 {
            return Err(EAGAIN);
        }
    }
}

pub fn socket_send_bytes(id: usize, data: &[u8], nonblock: bool) -> SysResult {
    let h = stream_handle(id)?;
    let mut sent = 0;
    loop {
        poll();
        let n = net();
        let s = n.sockets.get_mut::<tcp::Socket>(h);
        if !s.is_active() || !s.may_send() {
            return if sent > 0 { Ok(sent) } else { Err(32) }; // EPIPE
        }
        if s.can_send() {
            match s.send_slice(&data[sent..]) {
                Ok(n2) => {
                    sent += n2;
                    if sent == data.len() {
                        poll();
                        return Ok(sent);
                    }
                }
                Err(_) => return Err(ECONNRESET),
            }
        }
        if nonblock && sent > 0 {
            poll();
            return Ok(sent);
        }
        if nonblock {
            return Err(EAGAIN);
        }
    }
}

pub fn sendto(fd: usize, buf: usize, len: usize, flags: usize, _addr: usize, _alen: usize) -> SysResult {
    let id = sock_id(fd)?;
    let nonblock = current().fds.get(fd).map(|e| e.nonblock).unwrap_or(false)
        || flags & MSG_DONTWAIT != 0;
    let src = user_slice(buf, len)?;
    socket_send_bytes(id, src, nonblock)
}

pub fn recvfrom(fd: usize, buf: usize, len: usize, flags: usize, _addr: usize, _alen: usize) -> SysResult {
    let id = sock_id(fd)?;
    let nonblock = current().fds.get(fd).map(|e| e.nonblock).unwrap_or(false)
        || flags & MSG_DONTWAIT != 0;
    let f = if nonblock { flags | MSG_DONTWAIT } else { flags };
    socket_recv_flags(id, buf, len, f)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IoVec {
    base: usize,
    len: usize,
}

pub fn sendmsg(fd: usize, msg: usize, flags: usize) -> SysResult {
    let iov: usize = read_user(msg + 16)?;
    let iovlen: usize = read_user(msg + 24)?;
    let mut data = Vec::new();
    for i in 0..iovlen {
        let v: IoVec = read_user(iov + i * 16)?;
        if v.len > 0 {
            data.extend_from_slice(user_slice(v.base, v.len)?);
        }
    }
    let id = sock_id(fd)?;
    let nonblock = current().fds.get(fd).map(|e| e.nonblock).unwrap_or(false)
        || flags & MSG_DONTWAIT != 0;
    socket_send_bytes(id, &data, nonblock)
}

pub fn recvmsg(fd: usize, msg: usize, flags: usize) -> SysResult {
    let iov: usize = read_user(msg + 16)?;
    let iovlen: usize = read_user(msg + 24)?;
    let id = sock_id(fd)?;
    let nonblock = current().fds.get(fd).map(|e| e.nonblock).unwrap_or(false)
        || flags & MSG_DONTWAIT != 0;
    let f = if nonblock { flags | MSG_DONTWAIT } else { flags };
    let mut total = 0;
    for i in 0..iovlen {
        let v: IoVec = read_user(iov + i * 16)?;
        if v.len == 0 {
            continue;
        }
        match socket_recv_flags(id, v.base, v.len, f) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if n < v.len {
                    break;
                }
            }
            Err(e) => {
                if total > 0 {
                    break;
                }
                return Err(e);
            }
        }
    }
    Ok(total)
}

pub fn setsockopt(_fd: usize, _level: usize, _name: usize, _val: usize, _len: usize) -> SysResult {
    Ok(0)
}

pub fn getsockopt(_fd: usize, level: usize, name: usize, val: usize, len_ptr: usize) -> SysResult {
    // SOL_SOCKET=1, SO_ERROR=4
    if level == 1 && name == 4 {
        write_user(val, 0i32)?;
        write_user(len_ptr, 4u32)?;
        return Ok(0);
    }
    if val != 0 && len_ptr != 0 {
        write_user(val, 0i32)?;
        write_user(len_ptr, 4u32)?;
    }
    Ok(0)
}

pub fn shutdown(fd: usize, _how: usize) -> SysResult {
    let id = sock_id(fd)?;
    if let Ok(h) = stream_handle(id) {
        net().sockets.get_mut::<tcp::Socket>(h).close();
        poll();
    }
    Ok(0)
}

// ---------- epoll ----------

const EPOLLIN: u32 = 0x1;
const EPOLLOUT: u32 = 0x4;
const EPOLLERR: u32 = 0x8;
const EPOLLHUP: u32 = 0x10;
const EPOLLRDHUP: u32 = 0x2000;

pub fn epoll_create1(_flags: usize) -> SysResult {
    let n = net();
    let mut idx = None;
    for (i, e) in n.epolls.iter_mut().enumerate() {
        if e.is_none() {
            idx = Some(i);
            break;
        }
    }
    let id = match idx {
        Some(i) => {
            n.epolls[i] = Some(Epoll {
                interest: BTreeMap::new(),
            });
            i
        }
        None => {
            n.epolls.push(Some(Epoll {
                interest: BTreeMap::new(),
            }));
            n.epolls.len() - 1
        }
    };
    let fd = current().fds.alloc(crate::fs::FdEntry {
        obj: crate::fs::FdObj::Epoll(id),
        cloexec: true,
        nonblock: false,
    });
    Ok(fd)
}

fn epoll_id(epfd: usize) -> Result<usize, i32> {
    match current().fds.get(epfd).map(|e| &e.obj) {
        Some(crate::fs::FdObj::Epoll(id)) => Ok(*id),
        Some(_) => Err(EINVAL),
        None => Err(EBADF),
    }
}

pub fn epoll_ctl(epfd: usize, op: usize, fd: usize, event: usize) -> SysResult {
    let id = epoll_id(epfd)?;
    let n = net();
    let ep = n.epolls[id].as_mut().ok_or(EBADF)?;
    match op {
        1 | 3 => {
            // ADD | MOD  — epoll_event on riscv64: u32 events @0, u64 data @8
            let events: u32 = read_user(event)?;
            let data: u64 = read_user(event + 8)?;
            ep.interest.insert(fd, (events, data));
            Ok(0)
        }
        2 => {
            ep.interest.remove(&fd);
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

/// Compute current readiness of an fd (level-triggered).
fn readiness(fd: usize) -> u32 {
    let t = current();
    let Some(e) = t.fds.get(fd) else {
        return EPOLLERR | EPOLLHUP;
    };
    match &e.obj {
        crate::fs::FdObj::Socket(id) => {
            let n = net();
            let Some(ks) = n.ksocks[*id].as_ref() else {
                return EPOLLERR | EPOLLHUP;
            };
            match &ks.kind {
                Kind::TcpListen { .. } => {
                    let id = *id;
                    if pool_has_ready(net(), id) {
                        EPOLLIN
                    } else {
                        0
                    }
                }
                Kind::TcpStream { h, .. } => {
                    let s = net().sockets.get::<tcp::Socket>(*h);
                    let mut r = 0;
                    if s.can_recv() {
                        r |= EPOLLIN;
                    }
                    if s.can_send() {
                        r |= EPOLLOUT;
                    }
                    match s.state() {
                        tcp::State::CloseWait => r |= EPOLLIN | EPOLLRDHUP,
                        tcp::State::Closed => r |= EPOLLIN | EPOLLOUT | EPOLLHUP,
                        _ => {}
                    }
                    if !s.is_active() && s.state() != tcp::State::Listen {
                        r |= EPOLLHUP | EPOLLIN;
                    }
                    r
                }
                Kind::TcpNew { .. } => 0,
            }
        }
        crate::fs::FdObj::EventFd { val, .. } => {
            let mut r = EPOLLOUT;
            if *val.lock() > 0 {
                r |= EPOLLIN;
            }
            r
        }
        crate::fs::FdObj::File { .. } | crate::fs::FdObj::Null => EPOLLIN | EPOLLOUT,
        crate::fs::FdObj::Stdio => EPOLLOUT,
        _ => 0,
    }
}

pub fn epoll_pwait(epfd: usize, events: usize, maxevents: usize, timeout_ms: isize) -> SysResult {
    let id = epoll_id(epfd)?;
    let deadline = if timeout_ms >= 0 {
        Some(crate::time::now_ns() + timeout_ms as u64 * 1_000_000)
    } else {
        None
    };
    loop {
        poll();
        let interest: Vec<(usize, u32, u64)> = {
            let n = net();
            let ep = n.epolls[id].as_ref().ok_or(EBADF)?;
            ep.interest
                .iter()
                .map(|(fd, (ev, data))| (*fd, *ev, *data))
                .collect()
        };
        let mut count = 0;
        for (fd, ev, data) in interest {
            if count >= maxevents {
                break;
            }
            let r = readiness(fd) & (ev | EPOLLERR | EPOLLHUP);
            if r != 0 {
                // epoll_event: events u32 @0 (+4 pad), data u64 @8
                write_user(events + count * 16, r)?;
                write_user(events + count * 16 + 8, data)?;
                count += 1;
            }
        }
        if count > 0 {
            return Ok(count);
        }
        if let Some(d) = deadline {
            if crate::time::now_ns() >= d {
                return Ok(0);
            }
        }
        core::hint::spin_loop();
    }
}

// ---------- ppoll (minimal) ----------

pub fn ppoll(fds: usize, nfds: usize, timeout: usize) -> SysResult {
    // struct pollfd { int fd; short events; short revents; }
    let deadline = if timeout != 0 {
        let sec: i64 = read_user(timeout)?;
        let nsec: i64 = read_user(timeout + 8)?;
        Some(crate::time::now_ns() + sec as u64 * 1_000_000_000 + nsec as u64)
    } else {
        None
    };
    loop {
        poll();
        let mut count = 0;
        for i in 0..nfds {
            let fd: i32 = read_user(fds + i * 8)?;
            let events: u16 = read_user(fds + i * 8 + 4)?;
            if fd < 0 {
                write_user(fds + i * 8 + 6, 0u16)?;
                continue;
            }
            let r = readiness(fd as usize);
            // POLLIN=1 POLLOUT=4 map directly from EPOLLIN/EPOLLOUT
            let revents = (r & (events as u32 | 0x18)) as u16;
            write_user(fds + i * 8 + 6, revents)?;
            if revents != 0 {
                count += 1;
            }
        }
        if count > 0 {
            return Ok(count);
        }
        if let Some(d) = deadline {
            if crate::time::now_ns() >= d {
                return Ok(0);
            }
        }
    }
}
