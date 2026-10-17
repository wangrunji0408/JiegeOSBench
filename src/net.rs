// smoltcp-based TCP stack with a Linux sockets/epoll API surface.
use crate::arch;
use crate::fs;
use crate::println;
use crate::virtio;
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
pub const EPOLLRDHUP: u32 = 0x2000;

const POOL: usize = 24;
const RXBUF: usize = 32768;
const TXBUF: usize = 32768;

#[derive(Clone, Copy, PartialEq, Debug)]
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
    pending: [Option<SocketHandle>; 64],
    pool: [Option<SocketHandle>; POOL],
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

fn valid(h: SocketHandle) -> bool {
    sset().iter().any(|(x, _)| x == h)
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
        let n = len.min(1500);
        let mut fopt: Option<F> = Some(f);
        if self.ok && n + 10 <= 2048 && virtio::tx_can_send() {
            let mut out: Option<R> = None;
            let ok = virtio::tx_send(n + 10, |buf| {
                for b in buf[..10].iter_mut() {
                    *b = 0;
                }
                let ff = fopt.take().unwrap();
                out = Some(ff(&mut buf[10..]));
            });
            if ok.is_some() {
                return out.unwrap();
            }
        }
        let f = fopt.take().unwrap();
        unsafe {
            let s = &mut SCRATCH[..n];
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
    harvest();
    cleanup();
}

fn harvest() {
    let es = entries();
    for i in 0..es.len() {
        let mut e = match es[i] {
            Some(e) => e,
            None => continue,
        };
        if e.role != Role::Listener {
            continue;
        }
        let mut changed = false;
        for k in 0..POOL {
            if let Some(h) = e.pool[k] {
                if !valid(h) {
                    e.pool[k] = None;
                    changed = true;
                    continue;
                }
                let st = sset().get::<tcp::Socket>(h).state();
                if st != tcp::State::Listen {
                    if let Some(pk) = e.pending.iter().position(|p| p.is_none()) {
                        e.pending[pk] = Some(h);
                        e.pool[k] = None;
                        let nh = new_socket();
                        let ep = IpListenEndpoint {
                            addr: e.addr.map(IpAddress::Ipv4),
                            port: e.port,
                        };
                        if sset().get_mut::<tcp::Socket>(nh).listen(ep).is_ok() {
                            e.pool[k] = Some(nh);
                        } else {
                            sset().remove(nh);
                        }
                        changed = true;
                    }
                }
            }
        }
        for p in e.pending.iter_mut() {
            if let Some(h) = *p {
                if !valid(h) {
                    *p = None;
                    changed = true;
                    continue;
                }
                let st = sset().get::<tcp::Socket>(h).state();
                if matches!(
                    st,
                    tcp::State::Closed
                        | tcp::State::TimeWait
                        | tcp::State::LastAck
                        | tcp::State::Closing
                ) {
                    sset().remove(h);
                    *p = None;
                    changed = true;
                }
            }
        }
        if changed {
            es[i] = Some(e);
        }
    }
}

fn cleanup() {
    let es = entries();
    let mut remove: Vec<usize> = Vec::new();
    for (i, e) in es.iter().enumerate() {
        if let Some(e) = e {
            if e.closing {
                if !valid(e.handle) {
                    remove.push(i);
                    continue;
                }
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
    let rx: smoltcp::storage::RingBuffer<'static, u8> =
        smoltcp::storage::RingBuffer::new(vec![0u8; RXBUF]);
    let tx: smoltcp::storage::RingBuffer<'static, u8> =
        smoltcp::storage::RingBuffer::new(vec![0u8; TXBUF]);
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
    if let Some(ip) = ip {
        out[4..8].copy_from_slice(&ip.octets());
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
        pending: [None; 64],
        pool: [None; POOL],
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
    if unsafe { crate::TRACE } >= 2 {
        println!("[net] socket id={} fd={}", id, fd);
    }
    fd as isize
}

pub fn set_nonblock(id: u32, on: bool) {
    if let Some(e) = entry_mut(id) {
        e.nonblock = on;
    }
}

pub fn bind(id: u32, addr: &[u8]) -> isize {
    if unsafe { crate::TRACE } >= 2 {
        println!("[net] bind id={} addr={:?}", id, addr);
    }
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
    if e.port == 0 {
        return -22;
    }
    let ep = IpListenEndpoint {
        addr: e.addr.map(IpAddress::Ipv4),
        port: e.port,
    };
    if unsafe { crate::TRACE } >= 2 {
        println!("[net] listen id={} port={}", id, e.port);
    }
    let mut first: Option<SocketHandle> = None;
    for k in 0..POOL {
        let h = new_socket();
        if sset().get_mut::<tcp::Socket>(h).listen(ep).is_ok() {
            e.pool[k] = Some(h);
            if first.is_none() {
                first = Some(h);
            }
        } else {
            sset().remove(h);
        }
    }
    match first {
        Some(h) => {
            e.handle = h;
            e.role = Role::Listener;
            0
        }
        None => -22,
    }
}

impl SockEntry {
    fn audit_port(&self) -> u16 {
        self.port
    }
}

pub fn accept(id: u32, flags: u32, addr_out: &mut [u8]) -> isize {
    loop {
        poll();
        let e = match entry(id) {
            Some(e) => e,
            None => return -9,
        };
        if e.role != Role::Listener {
            return -22;
        }
        let mut chosen: Option<(usize, SocketHandle)> = None;
        for (k, p) in e.pending.iter().enumerate() {
            if let Some(h) = *p {
                if !valid(h) {
                    if let Some(em) = entry_mut(id) {
                        em.pending[k] = None;
                    }
                    continue;
                }
                let st = sset().get::<tcp::Socket>(h).state();
                if st == tcp::State::Established || st == tcp::State::CloseWait {
                    chosen = Some((k, h));
                    break;
                }
            }
        }
        if let Some((k, h)) = chosen {
            let sock = sset().get::<tcp::Socket>(h);
            let remote = sock.remote_endpoint().map(|ep| match ep.addr {
                IpAddress::Ipv4(a) => a,
                _ => Ipv4Address::new(0, 0, 0, 0),
            });
            let rport = sock.remote_endpoint().map(|ep| ep.port).unwrap_or(0);
            write_addr(addr_out, remote, rport);
            if let Some(em) = entry_mut(id) {
                em.pending[k] = None;
            }
            let nonblock = flags & 0x800 != 0;
            let nid = add_entry(SockEntry {
                handle: h,
                role: Role::Conn,
                port: e.port,
                addr: e.addr,
                nonblock,
                closing: false,
                pending: [None; 64],
                pool: [None; POOL],
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
            if unsafe { crate::TRACE } >= 3 {
                println!("[net] accept id={} new id={} fd={}", id, nid, fd);
            }
            return fd as isize;
        }
        if unsafe { crate::TRACE } >= 3 {
            crate::print!("[net] accept miss id={} nb={} states:", id, e.nonblock);
            for p in e.pending.iter() {
                match *p {
                    Some(h) => {
                        if valid(h) {
                            crate::print!(" {:?}", sset().get::<tcp::Socket>(h).state());
                        } else {
                            crate::print!(" BAD");
                        }
                    }
                    None => crate::print!(" -"),
                }
            }
            println!();
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

pub fn recv(id: u32, buf: &mut [u8], nonblock: bool) -> isize {
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
        if nonblock || e.nonblock {
            return -11;
        }
    }
}

pub fn recvfrom(id: u32, buf: &mut [u8], _addr_out: &mut [u8]) -> isize {
    recv(id, buf, false)
}

pub fn send(id: u32, buf: &[u8], nonblock: bool) -> isize {
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
        if nonblock || e.nonblock {
            return -11;
        }
    }
}

pub fn sendto(id: u32, buf: &[u8], _addr: &[u8]) -> isize {
    send(id, buf, false)
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
    if !valid(e.handle) && e.role != Role::Listener {
        if let Some(slot) = entries().get_mut(id as usize) {
            *slot = None;
        }
        return;
    }
    if e.role == Role::Listener {
        for p in e.pool.iter() {
            if let Some(h) = *p {
                sset().remove(h);
            }
        }
        for p in e.pending.iter() {
            if let Some(h) = *p {
                sset().remove(h);
            }
        }
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
        event[8], event[9], event[10], event[11], event[12], event[13], event[14], event[15],
    ]);
    if unsafe { crate::TRACE } >= 3 {
        println!("[net] epoll_ctl ep={} op={} fd={} ev={:#x} data={:#x}", id, op, fd, events, data);
    }
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
            let _ = want;
            if e.role == Role::Listener {
                let mut r = 0u32;
                for p in e.pending.iter() {
                    if let Some(h) = *p {
                        if !valid(h) {
                            continue;
                        }
                        let ps = sset().get::<tcp::Socket>(h).state();
                        if matches!(ps, tcp::State::Established | tcp::State::CloseWait) {
                            r |= EPOLLIN;
                        }
                        if matches!(ps, tcp::State::Closed | tcp::State::LastAck) {
                            r |= EPOLLERR | EPOLLHUP;
                        }
                    }
                }
                return r;
            }
            if !valid(e.handle) {
                return EPOLLERR;
            }
            let s = sset().get::<tcp::Socket>(e.handle);
            let st = s.state();
            let mut r = 0u32;
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
            if matches!(
                st,
                tcp::State::CloseWait
                    | tcp::State::LastAck
                    | tcp::State::Closing
                    | tcp::State::TimeWait
            ) {
                r |= EPOLLRDHUP;
            }
            if st == tcp::State::Closed {
                r |= EPOLLHUP | EPOLLERR;
            }
            r
        }
        fs::FdKind::EventFd(id) => {
            let evs = unsafe { &EVENTFDS };
            if let Some(Some(v)) = evs.get(id as usize) {
                if *v != 0 {
                    EPOLLIN
                } else {
                    0
                }
            } else {
                EPOLLERR
            }
        }
        fs::FdKind::SocketPair(id) => {
            let ps = unsafe { &PAIRS };
            if let Some(Some(e)) = ps.get(id as usize) {
                let mut r = 0;
                if !e.rq.is_empty() || e.closed || e.peer_closed {
                    r |= EPOLLIN;
                }
                if e.peer_closed {
                    r |= EPOLLRDHUP;
                }
                r
            } else {
                EPOLLERR
            }
        }
        fs::FdKind::PipeRead(_) | fs::FdKind::PipeWrite(_) => 0,
        _ => EPOLLIN,
    }
}

pub fn epoll_wait(id: u32, events: &mut [u8], maxevents: i32, timeout_ms: i32) -> isize {
    let deadline = if timeout_ms >= 0 {
        Some(now().total_micros() + (timeout_ms as i64) * 1000)
    } else {
        None
    };
    let mut last_report = now().total_micros();
    loop {
        poll();
        if unsafe { crate::TRACE } >= 4 {
            let t = now().total_micros();
            if t - last_report > 1_000_000 {
                last_report = t;
                let eps = unsafe { &mut EPOLLS };
                let es = entries();
                for (ei, ee) in es.iter().enumerate() {
                    if let Some(ee) = ee {
                        if ee.role == Role::Listener {
                            let mut line = alloc::string::String::new();
                            for k in 0..POOL {
                                match ee.pool[k] {
                                    Some(h) => {
                                        if valid(h) {
                                            line.push_str(&alloc::format!("{:?} ", sset().get::<tcp::Socket>(h).state()));
                                        } else {
                                            line.push_str("BAD ");
                                        }
                                    }
                                    None => line.push_str("- "),
                                }
                            }
                            let np = ee.pending.iter().filter(|p| p.is_some()).count();
                            println!("[net] listener {} pending={} pool: {}", ei, np, line);
                        }
                    }
                }
                if let Some(ep) = eps.get_mut(id as usize).and_then(|e| e.as_mut()) {
                    for ent in ep.list.iter() {
                        unsafe { println!("[net] wait fd={} ev={:#x} ready={:#x} rx={} tx={}", ent.fd, ent.events, fd_ready(ent.fd, ent.events), virtio::RX_COUNT, virtio::TX_COUNT); }
                    virtio::debug_dump();
                    }
                }
            }
        }
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
                    if unsafe { crate::TRACE } >= 3 {
                        let extra = match fs::file(ent.fd as usize) {
                            Some(f) => match f.kind {
                                fs::FdKind::Socket(id) => match entry(id) {
                                    Some(e) => {
                                        let st = if valid(e.handle) {
                                            sset().get::<tcp::Socket>(e.handle).state()
                                        } else {
                                            tcp::State::Closed
                                        };
                                        alloc::format!("sock id={} role={:?} st={:?}", id, e.role, st)
                                    }
                                    None => alloc::format!("sock id={} MISSING", id),
                                },
                                k => alloc::format!("kind={:?}", k),
                            },
                            None => alloc::format!("NOFILE"),
                        };
                        println!("[net] event fd={} r={:#x} {}", ent.fd, r, extra);
                    }
                    let off = n * 16;
                    events[off..off + 4].copy_from_slice(&r.to_ne_bytes());
                    events[off + 4..off + 8].copy_from_slice(&0u32.to_ne_bytes());
                    events[off + 8..off + 16].copy_from_slice(&ent.data.to_ne_bytes());
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

pub fn on_fd_close(fd: usize) {
    let eps = unsafe { &mut EPOLLS };
    for e in eps.iter_mut() {
        if let Some(ep) = e.as_mut() {
            ep.list.retain(|x| x.fd as usize != fd);
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

// ---- eventfd ----

static mut EVENTFDS: Vec<Option<u64>> = Vec::new();

pub fn eventfd(initval: u32, nonblock: bool) -> isize {
    let evs = unsafe { &mut EVENTFDS };
    let id = evs.len();
    evs.push(Some(initval as u64));
    let fd = fs::alloc_fd(fs::File {
        kind: fs::FdKind::EventFd(id as u32),
        node: 0,
        pos: 0,
        flags: if nonblock { fs::O_NONBLOCK } else { 0 },
    });
    if fd == usize::MAX {
        return -24;
    }
    fd as isize
}

pub fn eventfd_close(id: u32) {
    let evs = unsafe { &mut EVENTFDS };
    if let Some(s) = evs.get_mut(id as usize) {
        *s = None;
    }
}

pub fn eventfd_read(id: u32, buf: &mut [u8], nonblock: bool) -> isize {
    if buf.len() < 8 {
        return -22;
    }
    let evs = unsafe { &mut EVENTFDS };
    loop {
        if let Some(Some(v)) = evs.get_mut(id as usize) {
            if *v != 0 {
                let val = *v;
                *v = 0;
                buf[..8].copy_from_slice(&val.to_ne_bytes());
                return 8;
            }
        } else {
            return -9;
        }
        if nonblock {
            return -11;
        }
        poll();
    }
}

pub fn eventfd_write(id: u32, buf: &[u8]) -> isize {
    if buf.len() < 8 {
        return -22;
    }
    let v = u64::from_ne_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
    let evs = unsafe { &mut EVENTFDS };
    if let Some(Some(x)) = evs.get_mut(id as usize) {
        *x = x.wrapping_add(v);
        return 8;
    }
    -9
}

// ---- socketpair / pipe ----

struct Endpoint {
    rq: alloc::collections::VecDeque<u8>,
    peer: usize,
    closed: bool,
    peer_closed: bool,
}

static mut PAIRS: Vec<Option<Endpoint>> = Vec::new();

pub fn socketpair(_domain: i32, ty: i32, _proto: i32, sv: &mut [u8]) -> isize {
    if ty & 0xf != 1 {
        return -93;
    }
    let a = unsafe { PAIRS.len() };
    let b = a + 1;
    unsafe {
        PAIRS.push(Some(Endpoint { rq: alloc::collections::VecDeque::new(), peer: b, closed: false, peer_closed: false }));
        PAIRS.push(Some(Endpoint { rq: alloc::collections::VecDeque::new(), peer: a, closed: false, peer_closed: false }));
    }
    let fd1 = fs::alloc_fd(fs::File {
        kind: fs::FdKind::SocketPair(a as u32),
        node: 0,
        pos: 0,
        flags: 0,
    });
    let fd2 = fs::alloc_fd(fs::File {
        kind: fs::FdKind::SocketPair(b as u32),
        node: 0,
        pos: 0,
        flags: 0,
    });
    if fd1 == usize::MAX || fd2 == usize::MAX {
        return -24;
    }
    sv[0..4].copy_from_slice(&(fd1 as i32).to_ne_bytes());
    sv[4..8].copy_from_slice(&(fd2 as i32).to_ne_bytes());
    0
}

pub fn sp_close(id: u32) {
    let ps = unsafe { &mut PAIRS };
    let peer = match ps.get(id as usize).and_then(|e| e.as_ref()) {
        Some(e) => e.peer,
        None => return,
    };
    if let Some(Some(e)) = ps.get_mut(id as usize) {
        e.closed = true;
    }
    if let Some(Some(e)) = ps.get_mut(peer) {
        e.peer_closed = true;
    }
}

pub fn sp_read(id: u32, buf: &mut [u8], nonblock: bool) -> isize {
    let ps = unsafe { &mut PAIRS };
    loop {
        match ps.get_mut(id as usize).and_then(|e| e.as_mut()) {
            Some(e) => {
                if !e.rq.is_empty() {
                    let n = buf.len().min(e.rq.len());
                    for i in 0..n {
                        buf[i] = e.rq.pop_front().unwrap();
                    }
                    return n as isize;
                }
                if e.closed || e.peer_closed {
                    return 0;
                }
                if nonblock {
                    return -11;
                }
            }
            None => return -9,
        }
        poll();
    }
}

pub fn sp_write(id: u32, buf: &[u8]) -> isize {
    let ps = unsafe { &mut PAIRS };
    let peer = match ps.get(id as usize).and_then(|e| e.as_ref()) {
        Some(e) => e.peer,
        None => return -9,
    };
    match ps.get_mut(peer).and_then(|e| e.as_mut()) {
        Some(e) => {
            if e.closed {
                return -32;
            }
            for &b in buf {
                e.rq.push_back(b);
            }
            buf.len() as isize
        }
        None => -32,
    }
}
