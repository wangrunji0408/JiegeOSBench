//! smoltcp 集成 + 内核 HTTP 服务器（直接用 smoltcp，不经 syscall）。
//! QEMU user-net: guest 10.0.2.15/24, gw 10.0.2.2, host 8080->guest 80。

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::vec;
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, EthernetAddress, IpAddress, IpCidr, Ipv4Address};
use smoltcp::iface::{Interface, Config, SocketSet, SocketHandle};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State as TcpState};

const IFACE_IP: [u8; 4] = [10, 0, 2, 15];
const GATEWAY: [u8; 4] = [10, 0, 2, 2];
const LISTEN_PORT: u16 = 80;

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
            if got {
                crate::println!("[net] rx {} bytes", self.rx_buf.len());
            }
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
static mut LISTEN_HANDLE: Option<SocketHandle> = None;

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

        // 创建监听 socket
        let rx = Box::leak(Vec::with_capacity(8192).into_boxed_slice());
        let tx = Box::leak(Vec::with_capacity(8192).into_boxed_slice());
        let mut sock = TcpSocket::new(SocketBuffer::new(rx), SocketBuffer::new(tx));
        let _ = sock.listen(LISTEN_PORT);
        let h = SOCKETS.as_mut().unwrap().add(sock);
        LISTEN_HANDLE = Some(h);

        crate::println!("[net] smoltcp up @ 10.0.2.15, listening tcp/{}", LISTEN_PORT);
    }
    // 发送 gratuitous ARP 让 slirp 网关学习本机 MAC
    crate::net::send_gratuitous_arp();
    crate::net::send_gratuitous_arp();
    crate::net::dump_tx();
}

pub fn poll() {
    unsafe {
        if IFACE.is_none() {
            return;
        }
        crate::net::kick_rx();
        // 检查 TX 完成情况
        crate::net::poll_tx();
        let iface = IFACE.as_mut().unwrap();
        let dev = DEV.as_mut().unwrap();
        let sockets = SOCKETS.as_mut().unwrap();
        let _ = iface.poll(now(), dev, sockets);
    }
}

const HTTP_BODY: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 139\r\nConnection: close\r\n\r\n<!DOCTYPE html>\n<html><head><title>ijiege-os</title></head>\n<body><h1>Hello from nginx on a from-scratch RISC-V kernel!</h1></body></html>\n";

/// 内核 HTTP 服务器主循环
pub fn http_serve_step() {
    poll();
    let h = match unsafe { LISTEN_HANDLE } {
        Some(h) => h,
        None => return,
    };
    unsafe {
        let sockets = SOCKETS.as_mut().unwrap();
        let s = sockets.get_mut::<TcpSocket>(h);
        let st = s.state();
        match st {
            TcpState::Established => {
                if s.can_send() {
                    // 尝试读请求（消费掉，不解析）
                    let mut buf = [0u8; 512];
                    let _ = s.recv_slice(&mut buf);
                    // 发响应
                    let n = s.send_slice(HTTP_BODY.as_bytes()).unwrap_or(0);
                    crate::println!("[http] send_slice={}", n);
                    // 不立即 close，让数据先发出去；下次 poll 后再 close
                    let _ = s.close();
                }
            }
            TcpState::Closed => {
                s.abort();
                let _ = s.listen(LISTEN_PORT);
            }
            _ => {}
        }
    }
}
