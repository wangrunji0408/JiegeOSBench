//! The smoltcp interface and socket set.

use crate::drivers::virtio_net;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};
use spin::Mutex;

/// Our IP configuration. QEMU's user-mode network puts the guest at 10.0.2.15
/// with the gateway (and its DNS proxy) at 10.0.2.2, so we can use static
/// addressing and skip DHCP entirely.
pub const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
pub const GATEWAY_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);
pub const PREFIX_LEN: u8 = 24;

/// A smoltcp `Device` over our virtio-net driver.
pub struct VirtioDevice;

impl Device for VirtioDevice {
    type RxToken<'a> = VirtioRxToken;
    type TxToken<'a> = VirtioTxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        // The virtio queue depth bounds how many frames we can have in flight.
        caps.max_burst_size = Some(64);
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let frame = virtio_net::receive()?;
        Some((VirtioRxToken(frame), VirtioTxToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtioTxToken)
    }
}

pub struct VirtioRxToken(Vec<u8>);

impl RxToken for VirtioRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

pub struct VirtioTxToken;

impl TxToken for VirtioTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = alloc::vec![0u8; len];
        let result = f(&mut buffer);
        virtio_net::transmit(&buffer);
        result
    }
}

/// Everything the network stack owns, behind one lock.
pub struct NetStack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    device: VirtioDevice,
}

static STACK: Mutex<Option<NetStack>> = Mutex::new(None);

/// Convert kernel time to a smoltcp `Instant`.
fn now() -> Instant {
    Instant::from_micros(crate::time::monotonic_us() as i64)
}

pub fn init() {
    if !virtio_net::present() {
        crate::warn!("no network device found; sockets will not work");
        return;
    }
    let mac = virtio_net::mac_address().unwrap();
    let hw = EthernetAddress(mac);
    let mut device = VirtioDevice;
    let mut config = Config::new(HardwareAddress::Ethernet(hw));
    // A fixed random seed is fine; it only perturbs port and sequence choices.
    config.random_seed = crate::arch::cycle();

    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(GUEST_IP, PREFIX_LEN)));
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(GATEWAY_IP)
        .expect("cannot install default route");

    *STACK.lock() = Some(NetStack {
        iface,
        sockets: SocketSet::new(Vec::new()),
        device,
    });

    crate::info!(
        "network: {}/{} via {} (mac {})",
        GUEST_IP,
        PREFIX_LEN,
        GATEWAY_IP,
        hw
    );
}

/// Run `f` with the stack locked.
pub fn with_stack<T>(f: impl FnOnce(&mut NetStack) -> T) -> Option<T> {
    let mut guard = STACK.lock();
    guard.as_mut().map(f)
}

/// Is the network stack up?
pub fn is_up() -> bool {
    STACK.lock().is_some()
}

/// Add a socket to the set, returning its handle.
pub fn add_tcp_socket(rx_bytes: usize, tx_bytes: usize) -> Option<SocketHandle> {
    let rx_buffer = tcp::SocketBuffer::new(alloc::vec![0u8; rx_bytes]);
    let tx_buffer = tcp::SocketBuffer::new(alloc::vec![0u8; tx_bytes]);
    let socket = tcp::Socket::new(rx_buffer, tx_buffer);
    with_stack(|stack| stack.sockets.add(socket))
}

pub fn add_udp_socket(rx_bytes: usize, tx_bytes: usize) -> Option<SocketHandle> {
    // Metadata storage bounds how many separate datagrams can be buffered.
    let rx_buffer = udp::PacketBuffer::new(
        alloc::vec![udp::PacketMetadata::EMPTY; 32],
        alloc::vec![0u8; rx_bytes],
    );
    let tx_buffer = udp::PacketBuffer::new(
        alloc::vec![udp::PacketMetadata::EMPTY; 32],
        alloc::vec![0u8; tx_bytes],
    );
    let socket = udp::Socket::new(rx_buffer, tx_buffer);
    with_stack(|stack| stack.sockets.add(socket))
}

pub fn remove_socket(handle: SocketHandle) {
    with_stack(|stack| {
        stack.sockets.remove(handle);
    });
}

/// Run `f` with a TCP socket.
pub fn with_tcp<T>(handle: SocketHandle, f: impl FnOnce(&mut tcp::Socket<'static>) -> T) -> Option<T> {
    with_stack(|stack| {
        let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
        f(socket)
    })
}

pub fn with_udp<T>(handle: SocketHandle, f: impl FnOnce(&mut udp::Socket<'static>) -> T) -> Option<T> {
    with_stack(|stack| {
        let socket = stack.sockets.get_mut::<udp::Socket>(handle);
        f(socket)
    })
}

/// Drive the stack: process received frames and send queued data.
///
/// Called from the idle loop, from the virtio interrupt handler, and from
/// blocking socket operations.
pub fn poll() {
    let mut guard = STACK.lock();
    let Some(stack) = guard.as_mut() else {
        return;
    };
    let timestamp = now();
    // `poll` returns whether anything changed; loop while it keeps making
    // progress so a burst of frames is drained in one go, with a bound so a
    // pathological flood can't starve everything else.
    for _ in 0..16 {
        let NetStack {
            iface,
            sockets,
            device,
        } = stack;
        if !iface.poll(timestamp, device, sockets) {
            break;
        }
    }
}

/// Called from the virtio interrupt handler.
pub fn on_interrupt() {
    poll();
}

/// Poll and then wake any task waiting on socket readiness.
///
/// Blocking socket operations call this in their wait loop.
pub fn poll_and_yield() {
    poll();
    crate::task::yield_now();
    // Poll again after the yield so a frame that arrived while another task ran
    // is picked up before we re-check the condition.
    poll();
}

/// Ports currently claimed by a bound or listening socket.
///
/// smoltcp doesn't expose a way to enumerate listening endpoints, so we track
/// the allocation ourselves. `bind` claims a port and `Socket::drop` releases it.
static BOUND_PORTS: Mutex<alloc::collections::BTreeSet<u16>> =
    Mutex::new(alloc::collections::BTreeSet::new());

/// Claim `port`. Returns false if it is already taken.
pub fn claim_port(port: u16) -> bool {
    BOUND_PORTS.lock().insert(port)
}

pub fn release_port(port: u16) {
    BOUND_PORTS.lock().remove(&port);
}

pub fn is_port_bound(port: u16) -> bool {
    BOUND_PORTS.lock().contains(&port)
}

/// A free ephemeral port. smoltcp needs us to pick one for `connect` and for
/// `bind(0)`.
pub fn ephemeral_port() -> u16 {
    use core::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(32768);
    for _ in 0..(60999 - 32768) {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        if port < 32768 {
            NEXT.store(32768, Ordering::Relaxed);
            continue;
        }
        if claim_port(port) {
            return port;
        }
    }
    // Every ephemeral port is claimed; reuse the counter's value and let smoltcp
    // reject the collision rather than spinning forever.
    NEXT.load(Ordering::Relaxed)
}

/// Our IP address, for `getsockname` on an unbound socket.
pub fn local_ip() -> Ipv4Address {
    GUEST_IP
}

/// Is this address one of ours (or the wildcard)?
pub fn is_local_addr(addr: &IpAddress) -> bool {
    match addr {
        IpAddress::Ipv4(v4) => *v4 == GUEST_IP || v4.is_unspecified() || v4.is_loopback(),
    }
}
