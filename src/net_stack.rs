//! smoltcp 集成 + 用户态 socket 支持。
//! QEMU user-net: guest 10.0.2.15/24, gw 10.0.2.2, host 8080->guest 80。

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::vec;
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, EthernetAddress, IpAddress, IpCidr, Ipv4Address, IpEndpoint};
use smoltcp::iface::{Interface, Config, SocketSet, SocketHandle};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State as TcpState, RecvError};

const IFACE_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY: [u8; 4] = [10, 0, 2, 2];

pub struct NetDev {
    rx_buf: Vec<u8>,
    has_rx: bool,
}
impl NetDev {
    fn new() -> Self {
        Self { rx_buf: Vec::with_capacity(1514), has_rx: false }
    }
}

pub struct RxTok<'a> { data: &'a [u8] }
impl<'a> RxToken for RxTok<'a> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R { f(self.data) }
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

impl Device for NetDev {
    type RxToken<'b> = RxTok<'b> where Self: 'b;
    type TxToken<'b> = TxTok where Self: 'b;

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
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
            let data: &[u8] = unsafe {
                core::slice::from_raw_parts(self.rx_buf.as_ptr(), self.rx_buf.len())
            };
            Some((RxTok { data }, TxTok))
        } else {
            None
        }
    }
    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> { Some(TxTok) }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.max_transmission_unit = 1514;
        c.medium = Medium::Ethernet;
        c
    }
}

static mut DEV: Option<NetDev> = None;
static mut IFACE: Option<Interface> = None;
static mut SOCKETS: Option<SocketSet<'static>> = None;
static mut SOCK_MAP: Vec<Option<SocketHandle>> = Vec::new();

fn now() -> Instant {
    Instant::from_millis(crate::timer::ticks() as i64 * 10)
}

pub fn init() {
    unsafe {
        let mac = crate::net::driver().mac;
        let hw = HardwareAddress::Ethernet(EthernetAddress::from_bytes(&mac));
        let config = Config::new(hw);
        let dev = DEV.insert(NetDev::new());
        let now = now();
        let mut iface = Interface::new(config, dev, now);
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(
                IpAddress::v4(IFACE_IP[0], IFACE_IP[1], IFACE_IP[2], IFACE_IP[3]),
                24,
            ));
        });
        let _ = iface.routes_mut().add_default_ipv4_route(
            Ipv4Address::new(GATEWAY[0], GATEWAY[1], GATEWAY[2], GATEWAY[3]),
        );
        IFACE = Some(iface);
        SOCKETS = Some(SocketSet::new(Vec::new()));
        crate::println!("[net] smoltcp up @ 10.0.2.15/24 gw 10.0.2.2");
    }
    crate::net::send_gratuitous_arp();
    crate::net::send_gratuitous_arp();
}

/// 推进协议栈
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

fn tcp_buffer() -> SocketBuffer<'static> {
    let rx = Box::leak(vec![0u8; 16384].into_boxed_slice());
    let tx = Box::leak(vec![0u8; 16384].into_boxed_slice());
    SocketBuffer::new(rx, tx)
}

/// 创建一个 TCP socket，返回内核 sock id
pub fn new_tcp_socket() -> Option<usize> {
    unsafe {
        let sockets = SOCKETS.as_mut()?;
        let sock = TcpSocket::new(tcp_buffer(), tcp_buffer());
        let h = sockets.add(sock);
        // 存入映射
        let id = SOCK_MAP.len();
        SOCK_MAP.push(Some(h));
        Some(id)
    }
}

fn get_handle(id: usize) -> Option<SocketHandle> {
    unsafe { SOCK_MAP.get(id).copied().flatten() }
}

pub fn listen_socket(id: usize, port: u16) -> bool {
    unsafe {
        let sockets = match SOCKETS.as_mut() {
            Some(s) => s,
            None => return false,
        };
        let h = match get_handle(id) {
            Some(h) => h,
            None => return false,
        };
        let s = sockets.get_mut::<TcpSocket>(h);
        s.listen(port).is_ok()
    }
}

/// accept：阻塞直到有连接，返回新连接的 sock id
pub fn accept_socket(listen_id: usize) -> Option<usize> {
    loop {
        poll();
        unsafe {
            let sockets = SOCKETS.as_mut()?;
            let h = get_handle(listen_id)?;
            let s = sockets.get_mut::<TcpSocket>(h);
            if s.state() == TcpState::Established {
                // listen socket 接受了连接，本身变 Established
                // 创建新 socket id 占位，原 id 作为已连接流返回
                let new_id = new_tcp_socket()?;
                // 把原 handle 从映射移除，赋给新连接；新 socket 重新 listen
                // 这里：listen_id 的 handle 现在是已连接，返回它；new_id 用于后续 listen
                // 但调用方需要 new_id re-listen。简化：返回 listen_id 作为流，并标记
                return Some(listen_id);
            }
        }
    }
}

pub fn socket_send(id: usize, data: &[u8]) -> usize {
    loop {
        poll();
        unsafe {
            let sockets = SOCKETS.as_mut().unwrap();
            let h = match get_handle(id) {
                Some(h) => h,
                None => return 0,
            };
            let s = sockets.get_mut::<TcpSocket>(h);
            match s.send_slice(data) {
                Ok(n) => return n,
                Err(_) => {
                    if s.state() == TcpState::Closed {
                        return 0;
                    }
                }
            }
        }
    }
}

pub fn socket_recv(id: usize, buf: &mut [u8]) -> usize {
    loop {
        poll();
        unsafe {
            let sockets = SOCKETS.as_mut().unwrap();
            let h = match get_handle(id) {
                Some(h) => h,
                None => return 0,
            };
            let s = sockets.get_mut::<TcpSocket>(h);
            match s.recv_slice(buf) {
                Ok(n) => return n,
                Err(RecvError::Finished) => return 0,
                Err(_) => {
                    if s.state() == TcpState::Closed {
                        return 0;
                    }
                }
            }
        }
    }
}

pub fn socket_close(id: usize) {
    unsafe {
        if let Some(sockets) = SOCKETS.as_mut() {
            if let Some(h) = get_handle(id) {
                let s = sockets.get_mut::<TcpSocket>(h);
                let _ = s.close();
            }
        }
    }
}

pub fn remove_socket(id: usize) {
    unsafe {
        if let Some(sockets) = SOCKETS.as_mut() {
            if let Some(h) = get_handle(id) {
                sockets.remove(h);
            }
        }
        if id < SOCK_MAP.len() {
            SOCK_MAP[id] = None;
        }
    }
}
