/// 网络子系统
/// 使用smoltcp协议栈 + VirtIO网卡

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use lazy_static::lazy_static;

use smoltcp::{
    iface::{Config, Interface, SocketSet, Routes, SocketHandle},
    socket::{tcp, udp},
    time::Instant,
    wire::{
        EthernetAddress, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr,
        HardwareAddress,
    },
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
};

pub mod socket;

pub use socket::{SocketHandle as KernelSocketHandle, get_socket_by_fd, alloc_socket_fd};

/// 内核IP地址
pub static KERNEL_IP: Mutex<Option<Ipv4Address>> = Mutex::new(None);

pub struct VirtioDevice;

impl Device for VirtioDevice {
    type RxToken<'a> = VirtioRxToken where Self: 'a;
    type TxToken<'a> = VirtioTxToken where Self: 'a;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut guard = crate::drivers::net::NET_DEVICE.lock();
        if let Some(dev) = guard.as_mut() {
            dev.poll_rx();
            if let Some(pkt) = dev.rx_queue.pop_front() {
                return Some((
                    VirtioRxToken { data: pkt },
                    VirtioTxToken {},
                ));
            }
        }
        None
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtioTxToken {})
    }
}

pub struct VirtioRxToken {
    data: Vec<u8>,
}

impl RxToken for VirtioRxToken {
    fn consume<R, F>(self, f: F) -> R
    where F: FnOnce(&[u8]) -> R {
        f(&self.data)
    }
}

pub struct VirtioTxToken;

impl TxToken for VirtioTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where F: FnOnce(&mut [u8]) -> R {
        let mut buf = alloc::vec![0u8; len];
        let result = f(&mut buf);
        let mut guard = crate::drivers::net::NET_DEVICE.lock();
        if let Some(dev) = guard.as_mut() {
            dev.send(&buf);
        }
        result
    }
}

pub struct NetworkState {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub device: VirtioDevice,
}

unsafe impl Send for NetworkState {}
unsafe impl Sync for NetworkState {}

lazy_static! {
    pub static ref NETWORK: Mutex<Option<NetworkState>> = Mutex::new(None);
}

pub fn init() {
    let mac = {
        let guard = crate::drivers::net::NET_DEVICE.lock();
        if let Some(dev) = guard.as_ref() {
            dev.mac
        } else {
            println!("[net] No network device found");
            return;
        }
    };

    let ethernet_addr = EthernetAddress(mac);
    println!("[net] Ethernet address: {}", ethernet_addr);

    let config = Config::new(HardwareAddress::Ethernet(ethernet_addr));
    let mut device = VirtioDevice;

    let now = Instant::from_millis(crate::timer::get_time_ms() as i64);
    let mut iface = Interface::new(config, &mut device, now);

    // 设置静态IP：10.0.2.15/24（QEMU user network默认）
    let ip = Ipv4Address::new(10, 0, 2, 15);
    let gateway = Ipv4Address::new(10, 0, 2, 2);

    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(ip, 24))).ok();
    });

    iface.routes_mut().add_default_ipv4_route(gateway).ok();

    *KERNEL_IP.lock() = Some(ip);
    println!("[net] IP: {}/24, Gateway: {}", ip, gateway);

    let sockets = SocketSet::new(alloc::vec![]);

    *NETWORK.lock() = Some(NetworkState {
        iface,
        sockets,
        device,
    });

    socket::init();
}

/// 定期轮询网络
pub fn poll() {
    let mut guard = NETWORK.lock();
    if let Some(state) = guard.as_mut() {
        let now = Instant::from_millis(crate::timer::get_time_ms() as i64);
        state.iface.poll(now, &mut state.device, &mut state.sockets);
        socket::poll_sockets(&mut state.sockets);
    }
}

pub fn get_time() -> Instant {
    Instant::from_millis(crate::timer::get_time_ms() as i64)
}
