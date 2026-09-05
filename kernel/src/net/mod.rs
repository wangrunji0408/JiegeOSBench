//! Network stack: smoltcp interface on top of virtio-net.
pub mod socket;
pub mod tcp;
pub mod udp;
pub mod unix;

use alloc::sync::Weak;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

use crate::config::*;
use crate::drivers::virtio_net::{self, DEVICE};
use crate::sync::{Global, SpinLock};
use crate::task::wait::WaitQueue;

pub struct Stack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
}

pub static STACK: Global<SpinLock<Stack>> = Global::new();
/// Woken whenever socket state may have changed.
pub static NET_WQ: WaitQueue = WaitQueue::new();
static UP: AtomicBool = AtomicBool::new(false);
/// Live TCP socket objects (for edge-triggered epoll bookkeeping).
pub static TCP_SOCKETS: SpinLock<Vec<Weak<tcp::TcpSocket>>> = SpinLock::new(Vec::new());
/// Sockets closed by user space that still need to finish their FIN handshake.
pub static ORPHANS: SpinLock<Vec<smoltcp::iface::SocketHandle>> = SpinLock::new(Vec::new());

pub fn now() -> Instant {
    Instant::from_micros((crate::time::monotonic_ns() / 1000) as i64)
}

pub fn is_up() -> bool {
    UP.load(Ordering::Relaxed)
}

pub fn init() {
    if !virtio_net::init() {
        klog!("no network device found");
        return;
    }
    let mac = virtio_net::mac();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = crate::fs::devices::random_u64();
    let dev = DEVICE.get();
    let mut iface = Interface::new(config, dev, now());
    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(IpAddress::v4(IP_ADDR[0], IP_ADDR[1], IP_ADDR[2], IP_ADDR[3]), IP_PREFIX)).unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(GATEWAY[0], GATEWAY[1], GATEWAY[2], GATEWAY[3]))
        .unwrap();
    STACK.init(SpinLock::new(Stack { iface, sockets: SocketSet::new(Vec::new()) }));
    UP.store(true, Ordering::Relaxed);
    klog!(
        "net: ip {}.{}.{}.{}/{} gw {}.{}.{}.{}",
        IP_ADDR[0],
        IP_ADDR[1],
        IP_ADDR[2],
        IP_ADDR[3],
        IP_PREFIX,
        GATEWAY[0],
        GATEWAY[1],
        GATEWAY[2],
        GATEWAY[3]
    );
}

/// Run the stack: process incoming packets, send outgoing ones, wake waiters.
pub fn poll() {
    if !is_up() {
        return;
    }
    let Some(mut stack) = STACK.get().try_lock() else {
        // Re-entrant poll (e.g. interrupt during a socket op): skip.
        return;
    };
    let dev = DEVICE.get();
    let ts = now();
    let Stack { iface, sockets } = &mut *stack;
    let res = iface.poll(ts, dev, sockets);

    // Reap orphaned sockets that have fully closed.
    {
        let mut orphans = ORPHANS.lock();
        if !orphans.is_empty() {
            orphans.retain(|h| {
                let s = sockets.get::<smoltcp::socket::tcp::Socket>(*h);
                if s.state() == smoltcp::socket::tcp::State::Closed {
                    sockets.remove(*h);
                    false
                } else {
                    true
                }
            });
        }
    }

    let changed = matches!(res, smoltcp::iface::PollResult::SocketStateChanged);
    drop(stack);
    if changed {
        tcp::update_event_seqs();
        NET_WQ.wake_all();
    }
}
