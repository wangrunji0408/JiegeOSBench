//! smoltcp interface, socket table, and the implementation behind the public
//! `net::` socket API.
//!
//! Locking model: everything (device + interface + sockets + handle table)
//! lives in one `SpinLock<Option<Stack>>` (`super::STACK`).  `poll()` is the only
//! entry point allowed in interrupt context and therefore uses `try_lock`; if a
//! thread already holds the lock the interrupt handler just acknowledges the
//! device (see `virtio::Net::ack_interrupt_nolock`) and returns, leaving the
//! used-ring entries for the thread's own `poll()`.

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address,
};

use super::virtio::{self, Net};
use super::{ErrnoKind, TcpHandle, TcpState, GUEST_IP, GUEST_MAC};
use crate::sync::SpinLock;

/// Per-direction TCP socket buffer.  Generous, so a large HTTP response flows
/// with few wakeups.
pub const SOCK_BUF: usize = 64 * 1024;

/// Number of listening sockets kept for one port.  smoltcp 0.11 has no listen
/// backlog: a single listening socket can only complete one handshake at a
/// time and drops SYNs that arrive while it is busy, so we keep a pool.
const LISTEN_BACKLOG: usize = 16;

/// Ephemeral port range handed out by `tcp_connect` / `tcp_listen(0)`.
const EPHEMERAL_FIRST: u16 = 49152;
const EPHEMERAL_LAST: u16 = 60999;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Free,
    /// Listening socket; `port` is the listen port.
    Listener,
    /// Connected (or connecting) socket owned by a handle.
    Conn,
    /// Handle released while the FIN handshake is still running.  The socket
    /// stays in the set so the FIN can be retransmitted; `poll()` reaps it once
    /// it reaches Closed.  The handle itself is already invalid.
    Zombie,
}

struct Slot {
    kind: SlotKind,
    port: u16,
    sk: SocketHandle,
}

/// Everything behind `super::STACK`.
pub struct Stack {
    dev: Net,
    iface: Interface,
    sockets: SocketSet<'static>,
    slots: Vec<Slot>,
    next_port: u16,
}

static PRESENT: AtomicBool = AtomicBool::new(false);
static MAC: SpinLock<[u8; 6]> = SpinLock::new(GUEST_MAC);
/// PLIC interrupts seen (diagnostic; also proves the handler is wired up).
static IRQ_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now() -> Instant {
    Instant::from_micros((crate::time::uptime_ns() / 1_000) as i64)
}

fn ip_bytes(a: IpAddress) -> [u8; 4] {
    match a {
        IpAddress::Ipv4(v) => v.0,
    }
}

fn map_state(s: tcp::State) -> TcpState {
    match s {
        tcp::State::Closed => TcpState::Closed,
        tcp::State::Listen => TcpState::Listen,
        tcp::State::SynSent => TcpState::SynSent,
        tcp::State::SynReceived => TcpState::SynReceived,
        tcp::State::Established => TcpState::Established,
        tcp::State::FinWait1 => TcpState::FinWait1,
        tcp::State::FinWait2 => TcpState::FinWait2,
        tcp::State::CloseWait => TcpState::CloseWait,
        tcp::State::Closing => TcpState::Closing,
        tcp::State::LastAck => TcpState::LastAck,
        tcp::State::TimeWait => TcpState::TimeWait,
    }
}

fn new_socket() -> tcp::Socket<'static> {
    let mut s = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; SOCK_BUF]),
        tcp::SocketBuffer::new(vec![0u8; SOCK_BUF]),
    );
    // Immediate ACKs: this kernel serves request/response traffic.
    s.set_ack_delay(None);
    s
}

fn alloc_slot(slots: &mut Vec<Slot>, kind: SlotKind, port: u16, sk: SocketHandle) -> usize {
    for (i, s) in slots.iter_mut().enumerate() {
        if s.kind == SlotKind::Free {
            *s = Slot { kind, port, sk };
            return i;
        }
    }
    slots.push(Slot { kind, port, sk });
    slots.len() - 1
}

/// Drop sockets whose FIN handshake finished after the handle was released.
fn reap(slots: &mut [Slot], sockets: &mut SocketSet<'static>) {
    for s in slots.iter_mut() {
        if s.kind == SlotKind::Zombie
            && sockets.get::<tcp::Socket>(s.sk).state() == tcp::State::Closed
        {
            sockets.remove(s.sk);
            s.kind = SlotKind::Free;
        }
    }
}

/// Socket handle for a slot that must be a live connection.
fn conn_sk(st: &Stack, h: TcpHandle) -> Result<SocketHandle, ErrnoKind> {
    match st.slots.get(h.0) {
        Some(s) if s.kind == SlotKind::Conn => Ok(s.sk),
        Some(_) => Err(ErrnoKind::Invalid),
        None => Err(ErrnoKind::Invalid),
    }
}

impl Stack {
    fn alloc_port(&mut self) -> u16 {
        let p = self.next_port;
        self.next_port = if p >= EPHEMERAL_LAST {
            EPHEMERAL_FIRST
        } else {
            p + 1
        };
        p
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn init() {
    let mut found = None;
    for slot in 0..virtio::MAX_SLOTS {
        if let Some(dev) = Net::probe(slot) {
            found = Some(dev);
            break;
        }
    }
    let Some(mut dev) = found else {
        crate::println!("[net] no virtio-net device found (present=false)");
        return;
    };

    let mac = dev.mac;
    let base = dev.base;
    let slot = dev.slot;
    let features = dev.features;
    let dev_transport = dev.transport;
    let dev_hdr = dev.hdr_len;
    dev.publish_irq_base();

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = crate::time::uptime_ns()
        ^ crate::time::realtime_ms().wrapping_mul(0x9e37_79b9_7f4a_7c15);

    let mut iface = Interface::new(config, &mut dev, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(
            IpAddress::v4(GUEST_IP[0], GUEST_IP[1], GUEST_IP[2], GUEST_IP[3]),
            24,
        ));
    });
    let _ = iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(
            super::GATEWAY_IP[0],
            super::GATEWAY_IP[1],
            super::GATEWAY_IP[2],
            super::GATEWAY_IP[3],
        ));

    let sockets = SocketSet::new(Vec::new());
    *super::STACK.lock() = Some(Stack {
        dev,
        iface,
        sockets,
        slots: Vec::new(),
        next_port: EPHEMERAL_FIRST,
    });

    *MAC.lock() = mac;
    PRESENT.store(true, Ordering::Release);

    // virtio-mmio slot N is wired to PLIC IRQ N+1 on the QEMU virt machine.
    crate::plic::register(slot + 1, irq_handler);

    crate::println!(
        "[net] virtio-net at slot {} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} \
         base={:#x} transport={:?} hdr={} features={:#x} ip={}.{}.{}.{}",
        slot,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        base,
        dev_transport,
        dev_hdr,
        features,
        GUEST_IP[0],
        GUEST_IP[1],
        GUEST_IP[2],
        GUEST_IP[3],
    );
}

/// PLIC handler.  Runs with interrupts disabled, must never block: acknowledge
/// the device unconditionally, then opportunistically pump the stack.
fn irq_handler(_irq: usize) {
    IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    Net::ack_interrupt_nolock();
    poll();
}

pub fn poll() {
    let Some(mut g) = super::STACK.try_lock() else {
        // A thread is already inside the stack; it will drain the rings.
        return;
    };
    let Some(st) = g.as_mut() else {
        return;
    };

    st.dev.ack_interrupt();
    let now = now();
    let Stack {
        dev,
        iface,
        sockets,
        slots,
        ..
    } = st;
    iface.poll(now, dev, sockets);
    reap(slots, sockets);
}

pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

pub fn mac() -> [u8; 6] {
    *MAC.lock()
}

// ---------------------------------------------------------------------------
// TCP API
// ---------------------------------------------------------------------------

pub fn tcp_listen(port: u16) -> Result<TcpHandle, &'static str> {
    let mut g = super::STACK.lock();
    let st = g.as_mut().ok_or("network is down")?;

    // Port 0 means "pick an ephemeral port" (Linux auto-bind on listen()).
    let port = if port == 0 { st.alloc_port() } else { port };

    // Two listeners on one port would silently shadow each other: smoltcp
    // dispatches an incoming SYN to the first matching listening socket.
    if st
        .slots
        .iter()
        .any(|s| s.kind == SlotKind::Listener && s.port == port)
    {
        return Err("address already in use");
    }

    let mut first = None;
    for _ in 0..LISTEN_BACKLOG {
        let mut sock = new_socket();
        sock.listen(port).map_err(|_| "cannot listen on that port")?;
        let sk = st.sockets.add(sock);
        let idx = alloc_slot(&mut st.slots, SlotKind::Listener, port, sk);
        if first.is_none() {
            first = Some(idx);
        }
    }
    Ok(TcpHandle(first.unwrap()))
}

pub fn tcp_accept(h: TcpHandle) -> Result<Option<(TcpHandle, [u8; 4], u16)>, &'static str> {
    // The API contract: always pump the stack first so a caller looping with a
    // short sleep makes progress.
    poll();

    let mut g = super::STACK.lock();
    let st = g.as_mut().ok_or("network is down")?;
    let idx = h.0;

    let port = match st.slots.get(idx) {
        Some(s) if s.kind == SlotKind::Listener => s.port,
        _ => return Err("invalid handle"),
    };

    // Walk the whole listen pool for this port: any socket that left Listen
    // either completed a handshake (take it) or was reset (re-arm it).
    let candidates: Vec<usize> = st
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == SlotKind::Listener && s.port == port)
        .map(|(i, _)| i)
        .collect();

    for j in candidates {
        let sk = st.slots[j].sk;
        match st.sockets.get::<tcp::Socket>(sk).state() {
            tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived => continue,
            tcp::State::Closed => {
                st.sockets.remove(sk);
                let mut l = new_socket();
                match l.listen(port) {
                    Ok(()) => st.slots[j].sk = st.sockets.add(l),
                    Err(_) => st.slots[j].kind = SlotKind::Free,
                }
                continue;
            }
            _ => {
                let conn = match st.sockets.remove(sk) {
                    smoltcp::socket::Socket::Tcp(s) => s,
                };
                let (peer_ip, peer_port) = match conn.remote_endpoint() {
                    Some(ep) => (ip_bytes(ep.addr), ep.port),
                    None => ([0u8; 4], 0u16),
                };
                let mut l = new_socket();
                match l.listen(port) {
                    Ok(()) => {
                        st.slots[j].sk = st.sockets.add(l);
                        st.slots[j].kind = SlotKind::Listener;
                    }
                    Err(_) => st.slots[j].kind = SlotKind::Free,
                }
                let conn_sk = st.sockets.add(conn);
                let ni = alloc_slot(&mut st.slots, SlotKind::Conn, port, conn_sk);
                return Ok(Some((TcpHandle(ni), peer_ip, peer_port)));
            }
        }
    }
    Ok(None)
}

/// True when any socket in the listen pool for this handle's port has a
/// completed (acceptable) connection.
pub fn tcp_listen_pending(h: TcpHandle) -> bool {
    poll();
    let g = super::STACK.lock();
    let Some(st) = g.as_ref() else {
        return false;
    };
    let port = match st.slots.get(h.0) {
        Some(s) if s.kind == SlotKind::Listener => s.port,
        _ => return false,
    };
    st.slots.iter().any(|s| {
        s.kind == SlotKind::Listener
            && s.port == port
            && matches!(
                st.sockets.get::<tcp::Socket>(s.sk).state(),
                tcp::State::Established
                    | tcp::State::CloseWait
                    | tcp::State::Closing
                    | tcp::State::FinWait1
                    | tcp::State::FinWait2
                    | tcp::State::LastAck
            )
    })
}

pub fn tcp_connect(ip: [u8; 4], port: u16) -> Result<TcpHandle, &'static str> {
    if port == 0 {
        return Err("bad port");
    }
    let mut g = super::STACK.lock();
    let st = g.as_mut().ok_or("network is down")?;

    let local_port = st.alloc_port();
    let mut sock = new_socket();

    let Stack {
        dev: _,
        iface,
        sockets,
        slots,
        next_port: _,
    } = st;
    let cx = iface.context();
    let remote = IpEndpoint::new(
        IpAddress::v4(ip[0], ip[1], ip[2], ip[3]),
        port,
    );
    let local = IpListenEndpoint {
        addr: Some(IpAddress::v4(
            GUEST_IP[0],
            GUEST_IP[1],
            GUEST_IP[2],
            GUEST_IP[3],
        )),
        port: local_port,
    };
    sock.connect(cx, remote, local)
        .map_err(|_| "cannot connect")?;

    let sk = sockets.add(sock);
    let idx = alloc_slot(slots, SlotKind::Conn, local_port, sk);
    Ok(TcpHandle(idx))
}

pub fn tcp_send(h: TcpHandle, buf: &[u8]) -> Result<usize, ErrnoKind> {
    let mut g = super::STACK.lock();
    let Some(st) = g.as_mut() else {
        return Err(ErrnoKind::Invalid);
    };
    let sk = conn_sk(st, h)?;
    let sock = st.sockets.get_mut::<tcp::Socket>(sk);

    match sock.send_slice(buf) {
        Ok(n) => {
            if n == 0 && !buf.is_empty() {
                Err(ErrnoKind::Again)
            } else {
                Ok(n)
            }
        }
        Err(_) => Err(match sock.state() {
            tcp::State::Closed
            | tcp::State::TimeWait
            | tcp::State::Closing
            | tcp::State::LastAck
            | tcp::State::FinWait1
            | tcp::State::FinWait2 => ErrnoKind::Closed,
            tcp::State::Listen => ErrnoKind::Invalid,
            // SynSent / SynReceived: not open for data yet.
            _ => ErrnoKind::Again,
        }),
    }
}

pub fn tcp_recv(h: TcpHandle, buf: &mut [u8]) -> Result<usize, ErrnoKind> {
    let mut g = super::STACK.lock();
    let Some(st) = g.as_mut() else {
        return Err(ErrnoKind::Invalid);
    };
    let sk = conn_sk(st, h)?;
    let sock = st.sockets.get_mut::<tcp::Socket>(sk);
    let state = sock.state();

    if sock.can_recv() {
        return match sock.recv_slice(buf) {
            Ok(n) => Ok(n),
            Err(_) => Ok(0),
        };
    }
    if state == tcp::State::Closed {
        return Ok(0); // orderly closed
    }
    if sock.may_recv() {
        // Open connection, nothing buffered yet.
        return if buf.is_empty() {
            Ok(0)
        } else {
            Err(ErrnoKind::Again)
        };
    }
    match state {
        // Not a data-carrying state yet.
        tcp::State::Listen | tcp::State::SynSent | tcp::State::SynReceived => Err(ErrnoKind::Again),
        // CloseWait / Closing / LastAck / TimeWait with an empty buffer: EOF.
        _ => Ok(0),
    }
}

pub fn tcp_state(h: TcpHandle) -> TcpState {
    let g = super::STACK.lock();
    let Some(st) = g.as_ref() else {
        return TcpState::Closed;
    };
    match st.slots.get(h.0) {
        Some(s) if s.kind != SlotKind::Free => map_state(st.sockets.get::<tcp::Socket>(s.sk).state()),
        _ => TcpState::Closed,
    }
}

pub fn tcp_local_addr(h: TcpHandle) -> Option<([u8; 4], u16)> {
    let g = super::STACK.lock();
    let st = g.as_ref()?;
    let s = st.slots.get(h.0)?;
    match s.kind {
        SlotKind::Free => None,
        // A listening socket has no smoltcp tuple yet; report the bound port.
        SlotKind::Listener => Some((GUEST_IP, s.port)),
        _ => st
            .sockets
            .get::<tcp::Socket>(s.sk)
            .local_endpoint()
            .map(|ep| (ip_bytes(ep.addr), ep.port)),
    }
}

pub fn tcp_peer_addr(h: TcpHandle) -> Option<([u8; 4], u16)> {
    let g = super::STACK.lock();
    let st = g.as_ref()?;
    let s = st.slots.get(h.0)?;
    match s.kind {
        SlotKind::Free | SlotKind::Listener => None,
        _ => st
            .sockets
            .get::<tcp::Socket>(s.sk)
            .remote_endpoint()
            .map(|ep| (ip_bytes(ep.addr), ep.port)),
    }
}

pub fn tcp_shutdown(h: TcpHandle, read: bool, write: bool) {
    let mut g = super::STACK.lock();
    let Some(st) = g.as_mut() else {
        return;
    };
    let Ok(sk) = conn_sk(st, h) else {
        return;
    };
    if write {
        // Sends FIN; the socket stays alive for the closing handshake.
        st.sockets.get_mut::<tcp::Socket>(sk).close();
    }
    // smoltcp 0.11 has no half-close for the receive direction: there is no way
    // to tell the peer "I stopped reading" without aborting the connection.
    // `read` is therefore accepted and ignored; callers can drain with
    // tcp_recv() and then tcp_drop() to release the connection.
    let _ = read;
}

pub fn tcp_drop(h: TcpHandle) {
    let mut g = super::STACK.lock();
    let Some(st) = g.as_mut() else {
        return;
    };
    let Some(slot) = st.slots.get_mut(h.0) else {
        return;
    };
    match slot.kind {
        SlotKind::Free | SlotKind::Zombie => {}
        SlotKind::Listener => {
            let port = slot.port;
            for s in st.slots.iter_mut() {
                if s.kind == SlotKind::Listener && s.port == port {
                    st.sockets.remove(s.sk);
                    s.kind = SlotKind::Free;
                }
            }
        }
        SlotKind::Conn => {
            let sock = st.sockets.get_mut::<tcp::Socket>(slot.sk);
            if sock.state() == tcp::State::Closed {
                st.sockets.remove(slot.sk);
                slot.kind = SlotKind::Free;
            } else {
                // Graceful close: keep the socket around (but not the handle)
                // until the FIN handshake finishes, so the peer sees a clean
                // shutdown instead of an RST.
                sock.close();
                slot.kind = SlotKind::Zombie;
            }
        }
    }
}

pub fn tcp_pending(h: TcpHandle) -> usize {
    let g = super::STACK.lock();
    let Some(st) = g.as_ref() else {
        return 0;
    };
    let Some(s) = st.slots.get(h.0) else {
        return 0;
    };
    match s.kind {
        SlotKind::Free | SlotKind::Listener => 0,
        _ => st.sockets.get::<tcp::Socket>(s.sk).recv_queue(),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

pub fn selftest() {
    // Take the lock once: `MAC.lock()` twice in one expression would deadlock
    // (the first guard lives until the end of the statement).
    let mac = *MAC.lock();
    crate::println!(
        "[net] selftest: present={} irqs={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} \
         ip={}.{}.{}.{}",
        present(),
        IRQ_COUNT.load(Ordering::Relaxed),
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        GUEST_IP[0],
        GUEST_IP[1],
        GUEST_IP[2],
        GUEST_IP[3],
    );

    let Some(g) = super::STACK.try_lock() else {
        crate::println!("[net] selftest: stack lock busy");
        return;
    };
    let Some(st) = g.as_ref() else {
        crate::println!("[net] selftest: stack not initialised");
        return;
    };

    let d = &st.dev;
    crate::println!(
        "[net]   device: slot={} base={:#x} transport={:?} hdr_len={} status={:#x} \
         features={:#x} irq_status={:#x}",
        d.slot,
        d.base,
        d.transport,
        d.hdr_len,
        d.device_status(),
        d.features,
        d.last_irq_status,
    );
    crate::println!(
        "[net]   rx: num={} used_idx={} last_used={} packets={} malformed={}",
        d.rx.num(),
        d.rx.used_idx(),
        d.rx.last_used(),
        d.rx.packets,
        d.rx.malformed,
    );
    crate::println!(
        "[net]   tx: num={} free={} used_idx={} last_used={} packets={} dropped={}",
        d.tx.num(),
        d.tx.free(),
        d.tx.used_idx(),
        d.tx.last_used(),
        d.tx.packets,
        d.tx.dropped,
    );
    crate::println!(
        "[net]   sockets: {} handles ({} listening, {} connected, {} zombie)",
        st.slots.len(),
        st.slots.iter().filter(|s| s.kind == SlotKind::Listener).count(),
        st.slots.iter().filter(|s| s.kind == SlotKind::Conn).count(),
        st.slots.iter().filter(|s| s.kind == SlotKind::Zombie).count(),
    );
}
