//! smoltcp TCP/IP 栈集成（virtio-net 设备 + 10.0.2.15/24 静态配置）

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, SocketSet};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

use super::virtio_net::VirtioNet;

// QEMU user networking 静态配置
pub const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];

const LISTEN_POOL_SIZE: usize = 8;
const MAX_CONNECTIONS: usize = 96;
const TCP_BUF_SIZE: usize = 16 * 1024;

// 待发送帧队列（TxToken 与设备解耦）
static mut TX_PENDING: Vec<Vec<u8>> = Vec::new();
// smoltcp 需要一个从设备收来的帧的暂存
static mut RX_PENDING: Option<Vec<u8>> = None;

/// smoltcp Device 适配器
pub struct SmolDevice {
    pub net: VirtioNet,
}

pub struct SmolRxToken {
    frame: Vec<u8>,
}
pub struct SmolTxToken;

impl RxToken for SmolRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.frame)
    }
}

impl TxToken for SmolTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        unsafe {
            #[allow(static_mut_refs)]
            TX_PENDING.push(buf);
        }
        r
    }
}

impl Device for SmolDevice {
    type RxToken<'a> = SmolRxToken where Self: 'a;
    type TxToken<'a> = SmolTxToken where Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_,>, Self::TxToken<'_>)> {
        unsafe {
            #[allow(static_mut_refs)]
            {
                if RX_PENDING.is_none() {
                    RX_PENDING = self.net.receive();
                }
                if RX_PENDING.is_some() {
                    Some((SmolRxToken { frame: RX_PENDING.take().unwrap() }, SmolTxToken))
                } else {
                    None
                }
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.net.tx_available() {
            Some(SmolTxToken)
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(8);
        caps
    }
}

pub struct NetStack {
    pub device: SmolDevice,
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    /// 每个监听端口的 listen socket 池
    pub listeners: BTreeMap<u16, Vec<SocketHandle>>,
    /// 已 accept 的连接 handle -> 分配的 fd（用于回收判定）
    pub connections: BTreeMap<SocketHandle, usize>,
    /// 关闭中（等 FIN 握手完成）的 socket
    pub dying: Vec<SocketHandle>,
    /// 已 accept 关闭的 handle（等待 CLOSED 后移除）
    pub closed_handles: Vec<SocketHandle>,
}

static mut NET: Option<NetStack> = None;

pub fn net() -> &'static mut NetStack {
    unsafe {
        #[allow(static_mut_refs)]
        NET.as_mut().expect("network not initialized")
    }
}

pub fn init(virtio: Option<VirtioNet>) {
    let v = match virtio {
        Some(v) => v,
        None => {
            crate::kprintln!("net: no virtio-net device found, network disabled");
            return;
        }
    };
    let mac = v.mac();
    crate::kprintln!("net: virtio-net at MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    let device = SmolDevice { net: v };
    let now = Instant::from_millis(crate::trap::now_ms() as i64);
    let config = IfaceConfig::new(Medium::Ethernet);
    let mut iface = Interface::new(config, &mut SmolShadow(&device), now);
    iface.set_hardware_addr(HardwareAddress::Ethernet(EthernetAddress(mac)));
    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(IpAddress::v4(GUEST_IP[0], GUEST_IP[1], GUEST_IP[2], GUEST_IP[3]), 24));
    });
    let gw = Ipv4Address::new(GATEWAY_IP[0], GATEWAY_IP[1], GATEWAY_IP[2], GATEWAY_IP[3]);
    if iface.routes_mut().add_default_ipv4_route(gw).is_err() {
        crate::kprintln!("net: failed to set default route");
    }

    unsafe {
        #[allow(static_mut_refs)]
        {
            NET = Some(NetStack {
                device,
                iface,
                sockets: SocketSet::new(Vec::new()),
                listeners: BTreeMap::new(),
                connections: BTreeMap::new(),
                dying: Vec::new(),
                closed_handles: Vec::new(),
            });
        }
    }
    crate::kprintln!("net: interface up at 10.0.2.15/24 gw 10.0.2.2");
}

/// Interface::new 需要 &mut Device —— 用一个借用包装（构造期间用）
struct SmolShadow<'a>(&'a SmolDevice);
impl Device for SmolShadow<'_> {
    type RxToken<'a2> = SmolRxToken where Self: 'a2;
    type TxToken<'a2> = SmolTxToken where Self: 'a2;
    fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        None
    }
    fn transmit(&mut self, _t: Instant) -> Option<Self::TxToken<'_>> {
        None
    }
    fn capabilities(&self) -> DeviceCapabilities {
        self.0.capabilities()
    }
}

fn new_tcp_socket() -> tcp::Socket<'static> {
    tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; TCP_BUF_SIZE]),
        tcp::SocketBuffer::new(vec![0u8; TCP_BUF_SIZE]),
    )
}

/// 监听一个端口（构建 listen socket 池）
pub fn listen(port: u16) -> bool {
    if !initialized() {
        return false;
    }
    let n = net();
    let now = Instant::from_millis(crate::trap::now_ms() as i64);
    let entry = n.listeners.entry(port).or_default();
    if entry.is_empty() {
        for _ in 0..LISTEN_POOL_SIZE {
            let handle = n.sockets.add(new_tcp_socket());
            let sock = n.sockets.get_mut::<tcp::Socket>(handle);
            if sock.listen(port).is_err() {
                n.sockets.remove(handle);
                return false;
            }
        }
    }
    let _ = now;
    true
}

fn initialized() -> bool {
    unsafe {
        #[allow(static_mut_refs)]
        NET.is_some()
    }
}

/// 某端口是否有已建立连接等待 accept
pub fn has_established(port: u16) -> bool {
    if !initialized() {
        return false;
    }
    let n = net();
    if let Some(handles) = n.listeners.get(&port) {
        for h in handles {
            let sock = n.sockets.get::<tcp::Socket>(*h);
            if sock.state() == tcp::State::Established {
                return true;
            }
        }
    }
    false
}

/// 取走一个已建立的连接（accept 语义）
pub fn take_established(port: u16) -> Option<SocketHandle> {
    if !initialized() {
        return None;
    }
    let n = net();
    let found = {
        let handles = n.listeners.get(&port)?;
        handles
            .iter()
            .copied()
            .find(|h| n.sockets.get::<tcp::Socket>(*h).state() == tcp::State::Established)
    }?;
    let handles = n.listeners.get_mut(&port)?;
    handles.retain(|h| *h != found);
    // 补充一个新的 listen socket 保持池满
    let handle = n.sockets.add(new_tcp_socket());
    if n.sockets.get_mut::<tcp::Socket>(handle).listen(port).is_ok() {
        handles.push(handle);
    } else {
        n.sockets.remove(handle);
    }
    Some(found)
}

pub fn remove_listener(port: u16) {
    if !initialized() {
        return;
    }
    let n = net();
    if let Some(handles) = n.listeners.remove(&port) {
        for h in handles {
            n.sockets.remove(h);
        }
    }
}

/// 注册新 accept 的连接
pub fn register_connection(handle: SocketHandle, fd: usize) {
    let n = net();
    n.connections.insert(handle, fd);
}

/// 关闭一个连接 fd 对应的 socket（优雅 FIN）
pub fn close_connection(handle: SocketHandle) {
    let n = net();
    n.connections.remove(&handle);
    if let Some(sock) = n.sockets.get_mut::<tcp::Socket>(handle) {
        sock.close();
    }
    n.closed_handles.push(handle);
}

/// 轮询网络栈：处理收发包、推进 TCP 状态机
pub fn net_poll() {
    if !initialized() {
        return;
    }
    let now = Instant::from_millis(crate::trap::now_ms() as i64);
    let n = net();
    // 多轮 poll 直到收敛
    for _ in 0..32 {
        let worked = n.iface.poll(now, &mut n.device, &mut n.sockets);
        flush_tx();
        if !worked {
            break;
        }
    }
    // 再拉一次（可能 poll 后又来了包）
    flush_tx();

    // 清理已完全关闭的 handle
    let n = net();
    let mut still_closed = Vec::new();
    for h in n.closed_handles.drain(..) {
        let sock = n.sockets.get::<tcp::Socket>(h);
        if sock.state() == tcp::State::Closed {
            n.sockets.remove(h);
        } else {
            still_closed.push(h);
        }
    }
    n.closed_handles = still_closed;
}

fn flush_tx() {
    unsafe {
        #[allow(static_mut_refs)]
        {
            let n = net();
            for frame in TX_PENDING.drain(..) {
                n.device.net.send(&frame);
            }
        }
    }
}

/// 等待网络事件或超时（ms）。返回后调用方应 net_poll 并检查状态。
pub fn wait_ms(ms: u64) {
    let target = crate::trap::time_ticks() + ms * 10_000;
    crate::sbi::set_timer(target);
    loop {
        if crate::trap::time_ticks() >= target {
            return;
        }
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

/// 下一次需要 poll 的绝对时刻（ms），None 表示无定时任务
pub fn next_poll_delay_ms() -> Option<u64> {
    if !initialized() {
        return None;
    }
    let n = net();
    let now = Instant::from_millis(crate::trap::now_ms() as i64);
    n.iface
        .poll_at(now, &mut n.sockets)
        .map(|t| {
            let ms = t.total_millis() as u64;
            ms.saturating_sub(crate::trap::now_ms()).max(1)
        })
}

/// 连接数是否已达上限
pub fn connection_slots_available() -> bool {
    if !initialized() {
        return false;
    }
    let n = net();
    n.connections.len() + n.closed_handles.len() + n.dying.len() < MAX_CONNECTIONS
}
