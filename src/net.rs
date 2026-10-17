// smoltcp-based TCP stack with a Linux sockets/epoll API surface.
use crate::arch;
use crate::fs;
use crate::println;
use crate::virtio;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpListenEndpoint, Ipv4Address,
};

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;

const RXBUF: usize = 32768;
const TXBUF: usize = 32768;

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Unbound,
    Listener,
    Conn,
}

#[derive(Clone, Copy)]
struct SockEntry {
    handle: SocketHandle,
    role: Role,
    port: u16,
    addr: Option<Ipv4Address>,
    nonblock: bool,
    closing: bool,
}

struct EpollEntry {
    fd: i32,
    events: u32,
    data: u64,
}

struct Epoll {
    list: Vec<EpollEntry>,
}

static mut IFACE: Option<Interface> = None;
static mut SET: Option<SocketSet<'static>> = None;
static mut ENTRIES: Vec<Option<SockEntry>> = Vec::new();
static mut EPOLLS: Vec<Option<Epoll>> = Vec::new();
static mut PORT_NEXT: u16 = 49152;
static mut SCRATCH: [u8; 2048] = [0; 2048];

fn iface() -> &'static mut Interface {
    unsafe { IFACE.as_mut().unwrap() }
}
fn sset() -> &'static mut SocketSet<'static> {
    unsafe { SET.as_mut().unwrap() }
}
fn entries() -> &'static mut Vec<Option<SockEntry>> {
    unsafe { &mut ENTRIES }
}

fn now() -> Instant {
    Instant::from_micros((arch::rdtime() / 10) as i64)
}

struct VirtioDev;

struct RxTok {
    id: usize,
    ptr: usize,
    len: usize,
}

impl RxToken for RxTok {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let r = if self.len > 10 {
            let s = unsafe { core::slice::from_raw_parts((self.ptr + 10) as *const u8, self.len - 10) };
            f(s)
        } else {
            f(&[])
        };
        virtio::rx_release(self.id);
        r
    }
}

struct TxTok {
    ok: bool,
}

impl TxToken for TxTok {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        if self.ok && len + 10 <= 2048 {
            if let Some(r) = virtio::tx_send(len + 10, |buf| {
                for b in buf[..10].iter_mut() {
                    *b = 0;
                }
                f(&mut buf[10..])
            }) {
                return r;
            }
        }
        unsafe {
            let s = &mut SCRATCH[..len.min(2048)];
            f(s)
        }
    }
}

impl Device for VirtioDev {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok;

    fn receive(&mut self, _timestamp: Instant) -> Option<(RxTok, TxTok)> {
        let (id, ptr, len) = virtio::rx_pop()?;
        Some((
            RxTok { id, ptr, len },
            TxTok {
                ok: virtio::tx_can_send(),
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<TxTok> {
        if virtio::tx_can_send() {
            Some(TxTok { ok: true })
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ethernet;
        caps
    }
}

pub fn init() {
    let mac = virtio::init();
    if !virtio::present() {
        println!("[net] no virtio-net device; networking disabled");
        return;
    }
    let mut dev = VirtioDev;
    let mut cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    cfg.random_seed = arch::rdtime() ^ 0x5a5a1234;
    let mut ifc = Interface::new(cfg, &mut dev, now());
    ifc.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24));
    });
    let _ = ifc
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2));
    unsafe {
        IFACE = Some(ifc);
        SET = Some(SocketSet::new(Vec::new()));
        ENTRIES = Vec::new();
    }
    println!("[net] interface up: 10.0.2.15/24 gw 10.0.2.2");
}

pub fn poll() {
    unsafe {
        if IFACE.is_none() {
            return;
        }
    }
    let t = now();
    let dev = &mut VirtioDev;
    iface().poll(t, dev, sset());
    cleanup();
}

fn cleanup() {
    let es = entries();
    let mut remove: Vec<usize> = Vec::new();
    for (i, e) in es.iter().enumerate() {
        if let Some(e) = e {
            if e.closing {
                let st = sset().get::<tcp::Socket>(e.handle).state();
                if st == tcp::State::Closed {
                    remove.push(i);
                }
            }
        }
    }
    for i in remove {
        if let Some(e) = es[i] {
            sset().remove(e.handle);
            es[i] = None;
        }
    }
}

fn new_socket() -> SocketHandle {
    let rx = vec![0u8; RXBUF];
    let tx = vec![0u8; TXBUF];
    let rx: &'static mut [u8] = Box::leak(rx.into_boxed_slice());
    let tx: &'static mut [u8] = Box::leak(tx.into_boxed_slice());
    let s = tcp::Socket::new(rx, tx);
    sset().add(s)
}

fn add_entry(e: SockEntry) -> u32 {
    let es = entries();
    for (i, slot) in es.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(e);
            return i as u32;
        }
    }
    es.push(Some(e));
    (es.len() - 1) as u32
}

fn entry(id: u32) -> Option<SockEntry> {
    let es = entries();
    es.get(id as usize).and_then(|e| *e)
}

fn entry_mut(id: u32) -> Option<&'static mut SockEntry> {
    let es = entries();
    es.get_mut(id as usize).and_then(|e| e.as_mut())
}

fn parse_addr(addr: &[u8]) -> Option<(Option<Ipv4Address>, u16)> {
    if addr.len() < 8 {
        return None;
    }
    let family = u16::from_ne_bytes([addr[0], addr[1]]);
    if family != 2 {
        return None;
    }
    let port = u16::from_be_bytes([addr[2], addr[3]]);
    let ip = Ipv4Address::new(addr[4], addr[5], addr[6], addr[7]);
    let a = if ip.is_unspecified() { None } else { Some(ip) };
    Some((a, port))
}

fn write_addr(out: &mut [u8], ip: Option<Ipv4Address>, port: u16) {
    for b in out.iter_mut() {
        *b = 0;
    }
    if out.len() < 8 {
        return;
    }
    out[0..2].copy_from_slice(&2u16.to_ne_bytes());
    out[2..4].copy_from_slice(&port.to_be_bytes());
    match ip {
        Some(Ipv4Address::v4(a, b, c, d)) => {
            out[4] = a;
            out[5] = b;
            out[6] = c;
            out[7] = d;
        }
        _ => {}
    }
}

pub fn socket(domain: i32, ty: i32, _proto: i32) -> isize {
    if domain != 2 {
        return -97; // EAFNOSUPPORT
    }
    if ty & 0xf != 1 {
        return -93; // EPROTONOSUPPORT
    }
    poll();
    let h = new_socket();
    let nonblock = ty & 0x800 != 0;
    let id = add_entry(SockEntry {
        handle: h,
        role: Role::Unbound,
        port: 0,
        addr: None,
        nonblock,
        closing: false,
    });
    let fd = fs::alloc_fd(fs::File {
        kind: fs::FdKind::Socket(id),
        node: 0,
        pos: 0,
        flags: if nonblock { fs::O_NONBLOCK } else { 0 },
    });
    if fd == usize::MAX {
        return -24;
    }
    fd as isize
}

pub fn set_nonblock(id: u32, on: bool) {
    if let Some(e) = entry_mut(id) {
        e.nonblock = on;
    }
}

pub fn bind(id: u32, addr: &[u8]) -> isize {
    let (a, port) = match parse_addr(addr) {
        Some(v) => v,
        None => return -22,
    };
    match entry_mut(id) {
        Some(e) => {
            if e.role != Role::Unbound {
                return -22;
            }
            e.addr = a;
            e.port = port;
            0
        }
        None => -9,
    }
}

pub fn listen(id: u32, _backlog: i32) -> isize {
    let e = match entry_mut(id) {
        Some(e) => e,
        None => return -9,
    };
    if e.role == Role::Listener {
        return 0;
    }
    if e.audit_port() == 0 {
        return -22;
    }
    let ep = IpListenEndpoint {
        addr: e.addr.map(IpAddress::Ipv4),
        port: e.port,
    };
    let h = e.handle;
    match sset().get_mut::<tcp::Socket>(h).listen(ep) {
        Ok(()) => {
            e.role = Role::Listener;
            0
        }
        Err(_) => -22,
    }
}

impl SockEntry {
    fn audit_port(&self) -> u16 {
        self.port
    }
}

pub fn accept(id: u32, flags: u32, addr_out: &mut [u8]) -> isize {
    let e0 = match entry(id) {
        Some(e) => e,
        None => return -9,
    };
    if e0.role != Role::Listener {
        return -22;
    }
    loop {
        poll();
        let e = match entry(id) {
            Some(e) => e,
            None => return -9,
        };
        let st = sset().get::<tcp::Socket>(e.handle).state();
        if st == tcp::State::Established || st == tcp::State::CloseWait {
            let remote = sset()
                .get::<tcp::Socket>(e.handle)
                .remote_endpoint()
                .map(|ep| match ep.addr {
                    IpAddress::Ipv4(a) => a,
                    _ => Ipv4Address::new(0, 0, 0, 0),
                });
            let rport = sset()
                .get::<tcp::Socket>(e.handle)
                .remote_endpoint()
                .map(|ep| ep.port)
                .unwrap_or(0);
            write_addr(addr_out, remote, rport);
            // New listener for subsequent connections.
            let nh = new_socket();
            let ep = IpListenEndpoint {
                addr: e.addr.map(IpAddress::Ipv4),
                port: e.port,
            };
            if sset().get_mut::<tcp::Socket>(nh).listen(ep).is_err() {
                return -24;
            }
            let old = e.handle;
            if let Some(em) = entry_mut(id) {
                em.handle = nh;
            }
            let nonblock = flags & 0x800 != 0;
            let nid = add_entry(SockEntry {
                handle: old,
                role: Role::Conn,
                port: e.port,
                addr: e.addr,
                nonblock,
                closing: false,
            });
            let fd = fs::alloc_fd(fs::File {
                kind: fs::FdKind::Socket(nid),
                node: 0,
                pos: 0,
                flags: if nonblock { fs::O_NONBLOCK } else { 0 },
            });
            if fd == usize::MAX {
                return -24;
            }
            return fd as isize;
        }
        if e.nonblock {
            return -11;
        }
    }
}

pub fn connect(id: u32, addr: &[u8]) -> isize {
    let (a, port) = match parse_addr(addr) {
        Some(v) => v,
        None => return -22,
    };
    let ip = match a {
        Some(ip) => ip,
        None => return -22,
    };
    let lport = unsafe {
        PORT_NEXT = PORT_NEXT.wrapping_add(1);
        if PORT_NEXT < 49152 {
            PORT_NEXT = 49152;
        }
        PORT_NEXT
    };
    let e = match entry_mut(id) {
        Some(e) => e,
        None => return -9,
    };
    let h = e.handle;
    let remote = smoltcp::wire::IpEndpoint::new(IpAddress::Ipv4(ip), port);
    match sset()
        .get_mut::<tcp::Socket>(h)
        .connect(iface().context(), remote, lport)
    {
        Ok(()) => {
            e.role = Role::Conn;
            0
        }
        Err(_) => -111,
    }
}

pub fn recv(id: u32, buf: &mut [u8]) -> isize {
    loop {
        poll();
        let e = match entry(id) {
            Some(e) => e,
            None => return -9,
        };
        let s = sset().get_mut::<tcp::Socket>(e.handle);
        if s.can_recv() {
            return match s.recv_slice(buf) {
                Ok(n) => n as isize,
                Err(tcp::RecvError::Finished) => 0,
                Err(_) => -22,
            };
        }
        let st = s.state();
        if matches!(
            st,
            tcp::State::CloseWait
                | tcp::State::Closed
                | tcp::State::LastAck
                | tcp::State::TimeWait
                | tcp::State::Closing
        ) {
            return 0;
        }
        if e.nonblock {
            return -11;
        }
    }
}

pub fn recvfrom(id: u32, buf: &mut [u8], _addr_out: &mut [u8]) -> isize {
    recv(id, buf)
}

pub fn send(id: u32, buf: &[u8]) -> isize {
    loop {
        poll();
        let e = match entry(id) {
            Some(e) => e,
            None => return -9,
        };
        let s = sset().get_mut::<tcp::Socket>(e.handle);
        if !s.is_open() {
            return -32;
        }
        if s.can_send() {
            return match s.send_slice(buf) {
                Ok(n) => n as isize,
                Err(_) => -32,
            };
        }
        if e.nonblock {
            return -11;
        }
    }
}

pub fn sendto(id: u32, buf: &[u8], _addr: &[u8]) -> isize {
    send(id, buf)
}

pub fn shutdown_socket(id: u32, how: i32) -> isize {
    match entry(id) {
        Some(e) => {
            let s = sset().get_mut::<tcp::Socket>(e.handle);
            if how == 0 {
                0
            } else {
                s.close();
                0
            }
        }
        None => -9,
    }
}

pub fn getsockname(id: u32, out: &mut [u8]) -> isize {
    match entry(id) {
        Some(e) => {
            let (ip, port) = if e.role == Role::Listener {
                (e.addr, e.port)
            } else {
                let s = sset().get::<tcp::Socket>(e.handle);
                match s.local_endpoint() {
                    Some(ep) => (
                        Some(match ep.addr {
                            IpAddress::Ipv4(a) => a,
                            _ => Ipv4Address::new(0, 0, 0, 0),
                        }),
                        ep.port,
                    ),
                    None => (e.addr, e.port),
                }
            };
            let ip = ip.unwrap_or(Ipv4Address::new(10, 0, 2, 15));
            write_addr(out, Some(ip), port);
            16
        }
        None => -9,
    }
}

pub fn getpeername(id: u32, out: &mut [u8]) -> isize {
    match entry(id) {
        Some(e) => {
            let s = sset().get::<tcp::Socket>(e.handle);
            match s.remote_endpoint() {
                Some(ep) => {
                    let ip = match ep.addr {
                        IpAddress::Ipv4(a) => a,
                        _ => Ipv4Address::new(0, 0, 0, 0),
                    };
                    write_addr(out, Some(ip), ep.port);
                    16
                }
                None => -107,
            }
        }
        None => -9,
    }
}

pub fn setsockopt(_id: u32, _level: i32, _name: i32, _val: &[u8]) -> isize {
    0
}

pub fn getsockopt(_id: u32, level: i32, name: i32, out: &mut [u8]) -> isize {
    // SOL_SOCKET = 1
    if level == 1 {
        match name {
            3 => {
                // SO_TYPE
                if out.len() >= 4 {
                    out[0..4].copy_from_slice(&1i32.to_ne_bytes());
                }
                return 4;
            }
            4 => {
                // SO_ERROR
                if out.len() >= 4 {
                    out[0..4].copy_from_slice(&0i32.to_ne_bytes());
                }
                return 4;
            }
            7 | 8 => {
                if out.len() >= 4 {
                    out[0..4].copy_from_slice(&65536i32.to_ne_bytes());
                }
                return 4;
            }
            _ => {
                if out.len() >= 4 {
                    out[0..4].copy_from_slice(&0i32.to_ne_bytes());
                }
                return 4;
            }
        }
    }
    if out.len() >= 4 {
        out[0..4].copy_from_slice(&0i32.to_ne_bytes());
    }
    4
}

pub fn pending(id: u32) -> isize {
    match entry(id) {
        Some(e) => {
            let s = sset().get::<tcp::Socket>(e.handle);
            if s.can_recv() {
                s.recv_queue() as isize
            } else {
                0
            }
        }
        None => 0,
    }
}

pub fn close_socket(id: u32) {
    let e = match entry(id) {
        Some(e) => e,
        None => return,
    };
    if e.role == Role::Listener {
        sset().remove(e.handle);
        if let Some(slot) = entries().get_mut(id as usize) {
            *slot = None;
        }
        return;
    }
    sset().get_mut::<tcp::Socket>(e.handle).close();
    if let Some(em) = entry_mut(id) {
        em.closing = true;
    }
}

// ---- epoll ----

pub fn epoll_create() -> isize {
    let eps = unsafe { &mut EPOLLS };
    for (i, s) in eps.iter_mut().enumerate() {
        if s.is_none() {
            *s = Some(Epoll { list: Vec::new() });
            let fd = fs::alloc_fd(fs::File {
                kind: fs::FdKind::Epoll(i as u32),
                node: 0,
                pos: 0,
                flags: 0,
            });
            if fd == usize::MAX {
                return -24;
            }
            return fd as isize;
        }
    }
    eps.push(Some(Epoll { list: Vec::new() }));
    let i = eps.len() - 1;
    let fd = fs::alloc_fd(fs::File {
        kind: fs::FdKind::Epoll(i as u32),
        node: 0,
        pos: 0,
        flags: 0,
    });
    if fd == usize::MAX {
        return -24;
    }
    fd as isize
}

pub fn epoll_close(id: u32) {
    let eps = unsafe { &mut EPOLLS };
    if let Some(slot) = eps.get_mut(id as usize) {
        *slot = None;
    }
}

pub fn epoll_ctl(id: u32, op: i32, fd: i32, event: &[u8]) -> isize {
    let events = u32::from_ne_bytes([event[0], event[1], event[2], event[3]]);
    let data = u64::from_ne_bytes([
        event[4], event[5], event[6], event[7], event[8], event[9], event[10], event[11],
    ]);
    let eps = unsafe { &mut EPOLLS };
    let ep = match eps.get_mut(id as usize).and_then(|e| e.as_mut()) {
        Some(e) => e,
        None => return -9,
    };
    let pos = ep.list.iter().position(|e| e.fd == fd);
    match op {
        1 => {
            if pos.is_some() {
                return -17;
            }
            ep.list.push(EpollEntry { fd, events, data });
            0
        }
        2 => match pos {
            Some(p) => {
                ep.list.remove(p);
                0
            }
            None => -2,
        },
        3 => match pos {
            Some(p) => {
                ep.list[p].events = events;
                ep.list[p].data = data;
                0
            }
            None => -2,
        },
        _ => -22,
    }
}

fn fd_ready(fd: i32, want: u32) -> u32 {
    let f = match fs::file(fd as usize) {
        Some(f) => f,
        None => return EPOLLERR,
    };
    match f.kind {
        fs::FdKind::Socket(id) => {
            let e = match entry(id) {
                Some(e) => e,
                None => return EPOLLERR,
            };
            let s = sset().get::<tcp::Socket>(e.handle);
            let st = s.state();
            let mut r = 0u32;
            if e.role == Role::Listener {
                if matches!(st, tcp::State::Established | tcp::State::CloseWait) {
                    r |= EPOLLIN;
                }
            } else {
                if s.can_recv()
                    || matches!(
                        st,
                        tcp::State::CloseWait
                            | tcp::State::Closed
                            | tcp::State::LastAck
                            | tcp::State::Closing
                            | tcp::State::TimeWait
                    )
                {
                    r |= EPOLLIN;
                }
                if s.can_send() {
                    r |= EPOLLOUT;
                }
                if st == tcp::State::Closed {
                    r |= EPOLLHUP | EPOLLERR;
                }
            }
            let _ = want;
            r
        }
        _ => EPOLLIN,
    }
}

pub fn epoll_wait(id: u32, events: &mut [u8], maxevents: i32, timeout_ms: i32) -> isize {
    let deadline = if timeout_ms >= 0 {
        Some(now().total_micros() + (timeout_ms as i64) * 1000)
    } else {
        None
    };
    loop {
        poll();
        let mut n = 0usize;
        let maxn = maxevents.max(0) as usize;
        let eps = unsafe { &mut EPOLLS };
        if let Some(ep) = eps.get_mut(id as usize).and_then(|e| e.as_mut()) {
            for ent in ep.list.iter() {
                if n >= maxn {
                    break;
                }
                let r = fd_ready(ent.fd, ent.events);
                if r & (ent.events | EPOLLERR | EPOLLHUP) != 0 {
                    let off = n * 12;
                    events[off..off + 4].copy_from_slice(&r.to_ne_bytes());
                    events[off + 4..off + 12].copy_from_slice(&ent.data.to_ne_bytes());
                    n += 1;
                }
            }
        }
        if n > 0 {
            return n as isize;
        }
        if timeout_ms == 0 {
            return 0;
        }
        if let Some(d) = deadline {
            if now().total_micros() >= d {
                return 0;
            }
        }
    }
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
