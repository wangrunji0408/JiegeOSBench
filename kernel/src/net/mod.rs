//! Network stack: virtio-net device bridged into smoltcp, with a small
//! shim providing POSIX-style listen/accept semantics on top of smoltcp's
//! single-socket-per-listen model.

mod device;
mod listener;
pub mod socket;

use crate::drivers;
use crate::drivers::virtio_hal::VirtioHalImpl;
use alloc::vec::Vec;
use device::NetDevice;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use spin::{Mutex, Once};
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::mmio::MmioTransport;

const NET_QUEUE_SIZE: usize = 16;
const RX_BUFFER_LEN: usize = 2048;

pub type VirtioNetImpl = VirtIONet<VirtioHalImpl, MmioTransport, NET_QUEUE_SIZE>;

pub struct NetState {
    pub iface: Interface,
    pub device: NetDevice,
    pub sockets: SocketSet<'static>,
    pub listeners: Vec<listener::Listener>,
}

static NET: Once<Mutex<NetState>> = Once::new();

pub fn is_available() -> bool {
    NET.get().is_some()
}

pub fn init() {
    let Some(transport) = drivers::probe_net_transport() else {
        crate::println!("[net] no virtio-net device found; networking disabled");
        return;
    };
    let inner = match VirtioNetImpl::new(transport, RX_BUFFER_LEN) {
        Ok(net) => net,
        Err(e) => {
            crate::println!("[net] failed to initialize virtio-net: {:?}", e);
            return;
        }
    };
    let mac = inner.mac_address();
    let mut device = NetDevice { inner };
    let hw_addr = HardwareAddress::Ethernet(EthernetAddress(mac));
    let config = Config::new(hw_addr);
    let mut iface = Interface::new(config, &mut device, Instant::ZERO);
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
            .unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .unwrap();
    crate::println!(
        "[net] virtio-net up, mac={:02x?}, ip=10.0.2.15/24 via 10.0.2.2",
        mac
    );
    NET.call_once(|| {
        Mutex::new(NetState {
            iface,
            device,
            sockets: SocketSet::new(Vec::new()),
            listeners: Vec::new(),
        })
    });
}

fn now() -> Instant {
    let ticks = riscv::register::time::read64();
    Instant::from_millis((ticks / 10_000) as i64)
}

/// Drive the network stack: process incoming packets, let smoltcp update
/// socket states, service any pending accept queues. Safe/cheap to call
/// from any syscall that cares about socket readiness -- there is no
/// interrupt-driven wakeup, so polling here is how packets ever move.
pub fn poll() {
    let Some(net) = NET.get() else { return };
    let mut state = net.lock();
    let ts = now();
    state.iface.poll(ts, &mut state.device, &mut state.sockets);
    listener::service_listeners(&mut state);
}

pub fn with_net<R>(f: impl FnOnce(&mut NetState) -> R) -> Option<R> {
    let net = NET.get()?;
    Some(f(&mut net.lock()))
}
