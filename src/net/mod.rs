//! Networking: smoltcp interface + socket management over virtio-net.

pub mod virtio;

use crate::sync::SpinLock;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketSet, SocketHandle};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

pub struct NetStack {
    pub device: virtio::VirtioNet,
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
}

static NET: SpinLock<Option<NetStack>> = SpinLock::new(None);

pub fn now() -> Instant {
    Instant::from_millis((crate::sbi::get_time() / 1_000_000) as i64)
}

pub fn init() {
    let mut device = virtio::VirtioNet::init();
    let mac = device.mac();
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)).unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .unwrap();
    let sockets = SocketSet::new(vec![]);
    *NET.lock() = Some(NetStack {
        device,
        iface,
        sockets,
    });
    crate::println!("[net] virtio-net up, MAC {:02x?}, IP 10.0.2.15/24", mac);
}

/// Poll the network stack (drive packet processing).
pub fn poll() {
    let mut g = NET.lock();
    if let Some(net) = g.as_mut() {
        net.iface.poll(now(), &mut net.device, &mut net.sockets);
    }
}

fn tcp_socket() -> tcp::Socket<'static> {
    tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 65535]),
        tcp::SocketBuffer::new(vec![0; 65535]),
    )
}

/// Create a new socket; returns the SocketHandle's raw usize.
pub fn socket_new(is_tcp: bool) -> Result<usize, isize> {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    let h = if is_tcp {
        net.sockets.add(tcp_socket())
    } else {
        net.sockets.add(udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0; 65535]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0; 65535]),
        ))
    };
    Ok(h.0)
}

pub fn socket_listen(handle: usize, port: u16) -> isize {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    match net.sockets.get_mut::<tcp::Socket>(SocketHandle(handle)).listen(port) {
        Ok(()) => 0,
        Err(_) => -crate::syscall::EINVAL,
    }
}

/// Track bind() ports so listen() can use them (nginx binds then listens).
static BOUND_PORTS: SpinLock<Vec<(usize, u16)>> = SpinLock::new(Vec::new());

pub fn socket_bind(handle: usize, port: u16) -> isize {
    let mut b = BOUND_PORTS.lock();
    b.retain(|(h, _)| *h != handle);
    b.push((handle, port));
    0
}

pub fn socket_listen_stored(handle: usize) -> isize {
    let port = BOUND_PORTS
        .lock()
        .iter()
        .find(|(h, _)| *h == handle)
        .map(|(_, p)| *p)
        .unwrap_or(0);
    socket_listen(handle, port)
}

pub fn socket_connect(handle: usize, a: u8, b: u8, c: u8, d: u8, port: u16) -> isize {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    let remote = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(a, b, c, d)), port);
    let cx = net.iface.context();
    match net.sockets.get_mut::<tcp::Socket>(SocketHandle(handle)).connect(cx, remote, 0) {
        Ok(()) => 0,
        Err(_) => -crate::syscall::ECONNRESET,
    }
}

/// Accept a pending connection on a listening TCP socket.
/// Returns (conn_handle, new_listen_handle, peer_addr, peer_port).
pub fn socket_accept(handle: usize) -> Result<(usize, usize, [u8; 4], u16), isize> {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    net.iface.poll(now(), &mut net.device, &mut net.sockets);

    let (port, active, peer) = {
        let s = net.sockets.get::<tcp::Socket>(SocketHandle(handle));
        let port = s.local_endpoint().map(|e| e.port).unwrap_or(0);
        (port, s.is_active(), s.remote_endpoint())
    };
    if !active {
        return Err(-crate::syscall::EAGAIN);
    }

    // Allocate a new listening socket and put it back into listen state.
    let new_handle = net.sockets.add(tcp_socket());
    let _ = net.sockets.get_mut::<tcp::Socket>(new_handle).listen(port);

    let (pa, pp) = match peer {
        Some(ep) => match ep.addr {
            IpAddress::Ipv4(v4) => (v4.0, ep.port),
            _ => ([0, 0, 0, 0], 0),
        },
        None => ([0, 0, 0, 0], 0),
    };
    Ok((handle, new_handle.0, pa, pp))
}

pub fn socket_close(handle: usize) {
    let mut g = NET.lock();
    if let Some(net) = g.as_mut() {
        net.sockets.remove(SocketHandle(handle));
    }
}

pub fn socket_send(handle: usize, buf: &[u8]) -> isize {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    match net.sockets.get_mut::<tcp::Socket>(SocketHandle(handle)).send_slice(buf) {
        Ok(n) => n as isize,
        Err(_) => -crate::syscall::EAGAIN,
    }
}

pub fn socket_recv(handle: usize, buf: &mut [u8]) -> isize {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    match net.sockets.get_mut::<tcp::Socket>(SocketHandle(handle)).recv_slice(buf) {
        Ok(n) => n as isize,
        Err(_) => -crate::syscall::EAGAIN,
    }
}

/// Whether a socket has pending incoming data (readable) or can accept writes.
pub fn socket_readable(handle: usize) -> bool {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    net.iface.poll(now(), &mut net.device, &mut net.sockets);
    let s = net.sockets.get::<tcp::Socket>(SocketHandle(handle));
    s.is_active() || s.can_recv()
}

pub fn socket_writable(handle: usize) -> bool {
    let mut g = NET.lock();
    let net = g.as_mut().expect("net not initialized");
    net.sockets.get::<tcp::Socket>(SocketHandle(handle)).can_send()
}

/// Return the socket's local/remote endpoint.
pub fn socket_local(handle: usize) -> Option<(u32, u16)> {
    let g = NET.lock();
    let net = g.as_ref()?;
    let s = net.sockets.get::<tcp::Socket>(SocketHandle(handle));
    s.local_endpoint().map(|e| {
        let addr = match e.addr {
            IpAddress::Ipv4(v4) => u32::from_be_bytes(v4.0),
            _ => 0,
        };
        (addr, e.port)
    })
}

pub fn socket_peer(handle: usize) -> Option<(u32, u16)> {
    let g = NET.lock();
    let net = g.as_ref()?;
    let s = net.sockets.get::<tcp::Socket>(SocketHandle(handle));
    s.remote_endpoint().map(|e| {
        let addr = match e.addr {
            IpAddress::Ipv4(v4) => u32::from_be_bytes(v4.0),
            _ => 0,
        };
        (addr, e.port)
    })
}
