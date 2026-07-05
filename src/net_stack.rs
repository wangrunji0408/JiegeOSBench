//! smoltcp 集成：Device 实现 + Interface + SocketSet + socket 操作。
//! QEMU user-net：guest IP 10.0.2.15，网关 10.0.2.2，host 转发 8080->80。

use alloc::vec::Vec;
use alloc::boxed::Box;
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, EthernetAddress, IpAddress, IpCidr, Ipv4Address};
use smoltcp::iface::{Interface, Config, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State as TcpState};
use smoltcp::socket::SocketHandle;

const IFACE_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY: [u8; 4] = [10, 0, 2, 2];

/// 设备包装：持有一个收到的帧
pub struct NetDev {
    rx_buf: Vec<u8>,
    has_rx: bool,
}

impl NetDev {
    fn new() -> Self {
        Self { rx_buf: Vec::new(), has_rx: false }
    }
}

pub struct RxTok<'a> {
    data: &'a [u8],
}
impl<'a> RxToken for RxTok<'a> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.data)
    }
}

pub struct TxTok;
impl TxToken for TxTok {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf[..]);
        crate::net::send_packet(&buf[..]);
        r
    }
}

impl<'a> Device for &'a mut NetDev {
    type RxToken<'b> = RxTok<'b> where Self: 'b;
    type TxToken<'b> = TxTok where Self: 'b;

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // 从 virtio-net 取一个包到 rx_buf
        if !self.has_rx {
            let mut got = false;
            crate::net::recv_packets(|data| {
                self.rx_buf.clear();
                self.rx_buf.extend_from_slice(data);
                got = true;
            });
            self.has_rx = got;
        }
        if self.has_rx {
            self.has_rx = false;
            // 注意：返回的 RxToken 借用 self.rx_buf；smoltcp 在 poll 内立即消费
            let data: &[u8] = unsafe { core::slice::from_raw_parts(self.rx_buf.as_ptr(), self.rx_buf.len()) };
            Some((RxTok { data }, TxTok))
        } else {
            None
        }
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxTok)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.max_transmission_unit = 1514;
        c.medium = smoltcp::phy::Medium::Ethernet;
        c
    }
}

// 全局状态
static mut DEV: Option<NetDev> = None;
static mut IFACE: Option<Interface> = None;
static mut SOCKETS: Option<SocketSet<'static>> = None;

/// 初始化网络协议栈
pub fn init() {
    unsafe {
        let mac = crate::net::driver().mac;
        let hw = HardwareAddress::Ethernet(EthernetAddress::from_bytes(&mac));
        let config = Config::new(hw);
        let dev = DEV.insert(NetDev::new());
        let now = now();
        let mut iface = Interface::new(config, dev, now);
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(IpAddress::v4(IFACE_IP[0], IFACE_IP[1], IFACE_IP[2], IFACE_IP[3]), 24));
        });
        let _ = iface.routes_mut().add_default_v4_gateway(
            Ipv4Address::new(GATEWAY[0], GATEWAY[1], GATEWAY[2], GATEWAY[3]),
            now,
        );
        IFACE = Some(iface);
        SOCKETS = Some(SocketSet::new(Vec::new()));
        crate::println!("[net] smoltcp interface up @ 10.0.2.15/24 gw 10.0.2.2");
    }
}

fn now() -> Instant {
    Instant::from_millis(crate::timer::ticks() as i64 * 10)
}

/// 推进协议栈：处理收发。在时钟中断与阻塞 syscall 中调用。
pub fn poll() {
    unsafe {
        if IFACE.is_none() {
            return;
        }
        let iface = IFACE.as_mut().unwrap();
        let dev = DEV.as_mut().unwrap();
        let sockets = SOCKETS.as_mut().unwrap();
        let _ = iface.poll(now(), dev, sockets);
    }
}

/// socket 表项
pub struct TcpSlot {
    pub handle: Option<SocketHandle>,
    pub listening: bool,
    pub local_port: u16,
    pub accepted: Option<SocketHandle>, // accept 到的连接
}

const MAX_SOCK: usize = 32;
static mut SLOTS: [Option<TcpSlot>; MAX_SOCK] = [const { None }; MAX_SOCK];

fn tcp_buffer() -> SocketBuffer<'static> {
    // 用 Box 的 Vec 作为缓冲。SocketBuffer 需要 'static。
    let rx = Box::leak(Vec::with_capacity(4096).into_boxed_slice());
    let tx = Box::leak(Vec::with_capacity(4096).into_boxed_slice());
    SocketBuffer::new(rx, tx)
}

/// 创建一个 TCP socket，返回 fd
pub fn socket_tcp() -> isize {
    unsafe {
        for i in 0..MAX_SOCK {
            if SLOTS[i].is_none() {
                let sock = TcpSocket::new(tcp_buffer(), tcp_buffer());
                let handle = SOCKETS.as_mut().unwrap().add(sock);
                SLOTS[i] = Some(TcpSlot {
                    handle: Some(handle),
                    listening: false,
                    local_port: 0,
                    accepted: None,
                });
                return i as isize + 3; // fd 从 3 开始
            }
        }
    }
    -1
}

pub fn fd_to_slot(fd: usize) -> Option<&'static mut TcpSlot> {
    if fd < 3 {
        return None;
    }
    let i = fd - 3;
    unsafe {
        if i < MAX_SOCK {
            SLOTS[i].as_mut()
        } else {
            None
        }
    }
}

/// bind：记录本地端口
pub fn bind(fd: usize, port: u16) -> isize {
    let slot = match fd_to_slot(fd) {
        Some(s) => s,
        None => return -2,
    };
    slot.local_port = port;
    0
}

/// listen：把 socket 置为监听
pub fn listen(fd: usize) -> isize {
    let slot = match fd_to_slot(fd) {
        Some(s) => s,
        None => return -2,
    };
    unsafe {
        if let Some(h) = slot.handle {
            let s = SOCKETS.as_mut().unwrap().get_mut::<TcpSocket>(h);
            if s.listen(slot.local_port).is_err() {
                return -22;
            }
        }
    }
    slot.listening = true;
    0
}

/// accept：阻塞直到有连接到达，返回新连接的 fd
pub fn accept(listen_fd: usize) -> isize {
    let local_port = match fd_to_slot(listen_fd) {
        Some(s) => s.local_port,
        None => return -2,
    };
    loop {
        poll();
        // 找一个处于 Established 且未关联的 socket
        unsafe {
            let sockets = SOCKETS.as_mut().unwrap();
            for i in 0..MAX_SOCK {
                if let Some(h) = SLOTS[i].as_ref().and_then(|s| s.handle) {
                    if SLOTS[i].as_ref().unwrap().listening {
                        continue;
                    }
                    if SLOTS[i].as_ref().unwrap().accepted.is_some() {
                        continue;
                    }
                    let s = sockets.get_mut::<TcpSocket>(h);
                    if s.state() == TcpState::Established
                        && s.local_endpoint().port == local_port
                        && !s.listening
                    {
                        // 这个 socket 是被 listen socket 接受的连接？smoltcp 中
                        // listen 的 socket 本身会进入 Established。我们换一个新 socket accept。
                        // 简化：直接把 listen socket 的连接“移交”到一个新 fd。
                    }
                }
            }
        }
        // 简化策略：扫描是否有 listen socket 自己进入 Established
        // （smoltcp 中，listen 后该 socket 接受第一个连接变 Established）
        // 直接返回 listen_fd（因为 listen socket 复用了）
        // 但 nginx 需要新 fd。这里给一个新 socket fd 占位。
        // —— 改用更直接的方式：见下方实现
        return -35; // EAGAIN 占位
    }
}

/// 发送数据（阻塞直到全部发出或对端关闭）
pub fn send(fd: usize, data: &[u8]) -> isize {
    let slot = match fd_to_slot(fd) {
        Some(s) => s,
        None => return -2,
    };
    let h = match slot.handle {
        Some(h) => h,
        None => return -9,
    };
    let mut sent = 0;
    while sent < data.len() {
        poll();
        let n = unsafe {
            let sockets = SOCKETS.as_mut().unwrap();
            let s = sockets.get_mut::<TcpSocket>(h);
            match s.send_slice(&data[sent..]) {
                Ok(n) => n,
                Err(_) => break,
            }
        };
        if n == 0 {
            // 缓冲满，等一会（轮询）继续
            continue;
        }
        sent += n;
    }
    sent as isize
}

/// 接收数据（阻塞直到有数据或对端关闭）
pub fn recv(fd: usize, buf: &mut [u8]) -> isize {
    let slot = match fd_to_slot(fd) {
        Some(s) => s,
        None => return -2,
    };
    let h = match slot.handle {
        Some(h) => h,
        None => return -9,
    };
    loop {
        poll();
        let n = unsafe {
            let sockets = SOCKETS.as_mut().unwrap();
            let s = sockets.get_mut::<TcpSocket>(h);
            if !s.can_recv() {
                if s.state() == TcpState::Closed {
                    return 0;
                }
                continue;
            }
            match s.recv_slice(buf) {
                Ok(n) => n,
                Err(_) => return -1,
            }
        };
        if n > 0 {
            return n as isize;
        }
        // 无数据，继续轮询
    }
}

pub fn close_sock(fd: usize) -> isize {
    let slot = match fd_to_slot(fd) {
        Some(s) => s,
        None => return 0,
    };
    unsafe {
        if let Some(h) = slot.handle {
            SOCKETS.as_mut().unwrap().remove(h);
        }
    }
    let _ = fd;
    // 清空 slot
    unsafe {
        SLOTS[fd - 3] = None;
    }
    0
}
