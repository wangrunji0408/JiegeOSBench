//! 网络栈：smoltcp + virtio-net，socket 管理

use crate::drivers::virtio::with_net_device;
use crate::sync::UPIntrFreeCell;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address};

const GUEST_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

pub struct NetDevice;

impl phy::Device for NetDevice {
    type RxToken<'a> = NetRxToken;
    type TxToken<'a> = NetTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut buf = alloc::vec![0u8; 1600];
        let len = with_net_device(|d| d.recv(&mut buf)).flatten()?;
        buf.truncate(len);
        Some((NetRxToken(buf), NetTxToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(NetTxToken)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct NetRxToken(Vec<u8>);
pub struct NetTxToken;

impl phy::RxToken for NetRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

impl phy::TxToken for NetTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = alloc::vec![0u8; len];
        let r = f(&mut buf);
        with_net_device(|d| {
            let _ = d.send(&buf);
        });
        r
    }
}

pub struct NetStack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub device: NetDevice,
}

pub struct Sock {
    pub handle: SocketHandle,
    pub is_listener: bool,
    pub local_port: u16,
    pub local_addr: [u8; 4],
    /// 监听 socket 池（backlog）
    pub listen_pool: Vec<SocketHandle>,
}

lazy_static! {
    static ref NET_STACK: UPIntrFreeCell<Option<NetStack>> =
        unsafe { UPIntrFreeCell::new(None) };
    static ref SOCKETS: UPIntrFreeCell<Vec<Option<Sock>>> =
        unsafe { UPIntrFreeCell::new(Vec::new()) };
}

fn now() -> Instant {
    Instant::from_micros(crate::timer::get_time_us() as i64)
}

pub fn init() {
    let mac = with_net_device(|d| d.mac);
    if let Some(mac) = mac {
        let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        let mut device = NetDevice;
        let mut iface = Interface::new(config, &mut device, now());
        iface.update_ip_addrs(|addrs| {
            addrs.push(IpCidr::new(GUEST_IP.into(), 24)).unwrap();
        });
        iface.routes_mut().add_default_ipv4_route(GATEWAY).unwrap();
        *NET_STACK.lock() = Some(NetStack {
            iface,
            sockets: SocketSet::new(alloc::vec![]),
            device,
        });
        println!("net stack initialized: ip {} gw {}", GUEST_IP, GATEWAY);
    } else {
        println!("net stack disabled (no device)");
    }
}

/// 驱动网络栈（收包、TCP 状态机推进）
pub fn poll() {
    let mut guard = NET_STACK.lock();
    if let Some(stack) = guard.as_mut() {
        stack.iface.poll(now(), &mut stack.device, &mut stack.sockets);
    }
}

fn alloc_sock_id(sock: Sock) -> usize {
    let mut table = SOCKETS.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(sock);
            return i;
        }
    }
    table.push(Some(sock));
    table.len() - 1
}

fn new_tcp_socket(stack: &mut NetStack) -> SocketHandle {
    let rx_buf = tcp::SocketBuffer::new(alloc::vec![0u8; 16384]);
    let tx_buf = tcp::SocketBuffer::new(alloc::vec![0u8; 65536]);
    let sock = tcp::Socket::new(rx_buf, tx_buf);
    stack.sockets.add(sock)
}

/// socket(AF_INET, SOCK_STREAM)
pub fn tcp_socket() -> Option<usize> {
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut()?;
    let handle = new_tcp_socket(stack);
    Some(alloc_sock_id(Sock {
        handle,
        is_listener: false,
        local_port: 0,
        local_addr: [0; 4],
        listen_pool: Vec::new(),
    }))
}

pub fn bind(id: usize, addr: [u8; 4], port: u16) -> i32 {
    let mut table = SOCKETS.lock();
    match table.get_mut(id).and_then(|s| s.as_mut()) {
        Some(sock) => {
            sock.local_port = port;
            sock.local_addr = addr;
            0
        }
        None => -9, // EBADF
    }
}

const LISTEN_POOL_SIZE: usize = 64;

pub fn listen(id: usize, _backlog: i32) -> i32 {
    let (port, already) = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => (s.local_port, s.is_listener),
            None => return -9,
        }
    };
    if already {
        // 重复 listen：幂等返回成功
        return 0;
    }
    let handle = {
        let table = SOCKETS.lock();
        table.get(id).and_then(|s| s.as_ref()).map(|s| s.handle)
    };
    let handle = match handle {
        Some(h) => h,
        None => return -9,
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    // 清理旧的监听池（listen 可能被重复调用）
    let old_pool = {
        let mut table = SOCKETS.lock();
        match table.get_mut(id).and_then(|s| s.as_mut()) {
            Some(s) => core::mem::take(&mut s.listen_pool),
            None => return -9,
        }
    };
    for h in old_pool {
        if h != handle {
            let socket = stack.sockets.get_mut::<tcp::Socket>(h);
            socket.abort();
            stack.sockets.remove(h);
        }
    }
    // 主 socket + 池 socket 全部监听同一端口
    let mut pool = alloc::vec![handle];
    for _ in 0..LISTEN_POOL_SIZE - 1 {
        pool.push(new_tcp_socket(stack));
    }
    let mut ok = true;
    for &h in &pool {
        let socket = stack.sockets.get_mut::<tcp::Socket>(h);
        if socket.listen(port).is_err() {
            ok = false;
        }
    }
    if !ok {
        return -22;
    }
    let mut table = SOCKETS.lock();
    if let Some(s) = table.get_mut(id).and_then(|s| s.as_mut()) {
        s.is_listener = true;
        s.listen_pool = pool;
    }
    0
}

/// accept：监听 socket 池中有已建立的连接时，返回 (新 socket id, 远端 ip, 端口)
pub fn accept(id: usize) -> Result<(usize, [u8; 4], u16), i32> {
    let (pool, port) = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) if s.is_listener => (s.listen_pool.clone(), s.local_port),
            Some(_) => return Err(-22), // EINVAL
            None => return Err(-9),
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    // 找一个已建立连接的监听 socket
    let mut found: Option<SocketHandle> = None;
    for &h in &pool {
        let socket = stack.sockets.get_mut::<tcp::Socket>(h);
        if socket.is_active() {
            found = Some(h);
            break;
        }
    }
    let handle = match found {
        Some(h) => h,
        None => return Err(-11), // EAGAIN
    };
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    let remote = socket.remote_endpoint();
    // 用新 socket 补回监听池
    let new_handle = new_tcp_socket(stack);
    let new_socket = stack.sockets.get_mut::<tcp::Socket>(new_handle);
    new_socket.listen(port).ok();
    {
        let mut table = SOCKETS.lock();
        if let Some(s) = table.get_mut(id).and_then(|s| s.as_mut()) {
            for h in s.listen_pool.iter_mut() {
                if *h == handle {
                    *h = new_handle;
                }
            }
        }
    }
    let (ip, rport) = match remote {
        Some(ep) => {
            let ip = match ep.addr {
                smoltcp::wire::IpAddress::Ipv4(v4) => {
                    let b = v4.as_bytes();
                    [b[0], b[1], b[2], b[3]]
                }
                _ => [0; 4],
            };
            (ip, ep.port)
        }
        None => ([0; 4], 0),
    };
    let new_id = alloc_sock_id(Sock {
        handle,
        is_listener: false,
        local_port: port,
        local_addr: [0; 4],
        listen_pool: Vec::new(),
    });
    Ok((new_id, ip, rport))
}

/// 接收数据。Ok(n)>0 数据，Ok(0) EOF，Err(-11) EAGAIN
pub fn recv(id: usize, buf: &mut [u8]) -> Result<usize, i32> {
    let handle = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => s.handle,
            None => return Err(-9),
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    if socket.can_recv() {
        match socket.recv_slice(buf) {
            Ok(n) => Ok(n),
            Err(_) => Err(-11),
        }
    } else if !socket.may_recv() {
        Ok(0) // 对端关闭
    } else {
        Err(-11) // EAGAIN
    }
}

/// 查看接收队列中的数据量
pub fn recv_available(id: usize) -> usize {
    let handle = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => s.handle,
            None => return 0,
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    socket.recv_queue()
}

pub fn send(id: usize, data: &[u8]) -> Result<usize, i32> {
    let handle = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => s.handle,
            None => return Err(-9),
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    if !socket.may_send() {
        return Err(-32); // EPIPE
    }
    if socket.can_send() {
        match socket.send_slice(data) {
            Ok(n) => Ok(n),
            Err(_) => Err(-11),
        }
    } else {
        Err(-11)
    }
}

/// (readable, writable, error)
pub fn poll_socket(id: usize) -> (bool, bool, bool) {
    let (handle, is_listener, pool) = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => (s.handle, s.is_listener, s.listen_pool.clone()),
            None => return (false, false, true),
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    if is_listener {
        for h in pool {
            let socket = stack.sockets.get_mut::<tcp::Socket>(h);
            if socket.is_active() {
                return (true, false, false);
            }
        }
        return (false, false, false);
    }
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    {
        let readable = socket.can_recv() || !socket.may_recv();
        let writable = socket.can_send();
        let err = !socket.may_recv() && !socket.may_send();
        (readable, writable, err)
    }
}

pub fn shutdown(id: usize, how: i32) -> i32 {
    let handle = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => s.handle,
            None => return -9,
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    match how {
        0 => socket.close(), // SHUT_RD → 近似 close
        1 => socket.close(), // SHUT_WR
        _ => socket.abort(), // SHUT_RDWR
    }
    0
}

pub fn close_socket(id: usize) {
    let (handle, pool) = {
        let mut table = SOCKETS.lock();
        match table.get_mut(id) {
            Some(slot) => match slot.take() {
                Some(s) => (s.handle, s.listen_pool.clone()),
                None => return,
            },
            None => return,
        }
    };
    let mut guard = NET_STACK.lock();
    if let Some(stack) = guard.as_mut() {
        let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
        socket.abort();
        stack.sockets.remove(handle);
        for h in pool {
            if h != handle {
                let socket = stack.sockets.get_mut::<tcp::Socket>(h);
                socket.abort();
                stack.sockets.remove(h);
            }
        }
    }
}

pub fn set_nodelay(id: usize, on: bool) {
    let handle = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => s.handle,
            None => return,
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    socket.set_nagle_enabled(!on);
}

pub fn getsockname(id: usize) -> Option<([u8; 4], u16)> {
    let (handle, port) = {
        let table = SOCKETS.lock();
        match table.get(id).and_then(|s| s.as_ref()) {
            Some(s) => (s.handle, s.local_port),
            None => return None,
        }
    };
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    let ep = socket.local_endpoint();
    let g = GUEST_IP.as_bytes();
    Some(([g[0], g[1], g[2], g[3]], if port != 0 { port } else { ep.map(|e| e.port).unwrap_or(0) }))
}

pub fn getpeername(id: usize) -> Option<([u8; 4], u16)> {
    let handle = {
        let table = SOCKETS.lock();
        table.get(id).and_then(|s| s.as_ref()).map(|s| s.handle)
    }?;
    let mut guard = NET_STACK.lock();
    let stack = guard.as_mut().unwrap();
    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    socket.remote_endpoint().map(|ep| {
        let ip = match ep.addr {
            smoltcp::wire::IpAddress::Ipv4(v4) => {
                let b = v4.as_bytes();
                [b[0], b[1], b[2], b[3]]
            }
            _ => [0; 4],
        };
        (ip, ep.port)
    })
}
