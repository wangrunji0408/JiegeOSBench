use crate::config::*;
use crate::frame;
use crate::time;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::NonNull;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

// ---------------- HAL (identity-mapped physical memory) ----------------

pub struct HalImpl;

unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _dir: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let pa = frame::alloc_contig(pages);
        (pa, NonNull::new(pa as *mut u8).unwrap())
    }
    unsafe fn dma_dealloc(_pa: PhysAddr, _va: NonNull<u8>, _pages: usize) -> i32 {
        0
    }
    unsafe fn mmio_phys_to_virt(pa: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(pa as *mut u8).unwrap()
    }
    unsafe fn share(buffer: NonNull<[u8]>, _dir: BufferDirection) -> PhysAddr {
        buffer.as_ptr() as *mut u8 as usize
    }
    unsafe fn unshare(_pa: PhysAddr, _buffer: NonNull<[u8]>, _dir: BufferDirection) {}
}

const QS: usize = 16;
const NET_BUF_LEN: usize = 2048;
type Dev = VirtIONet<HalImpl, MmioTransport, QS>;

static mut NETDEV: Option<Dev> = None;

fn netdev() -> &'static mut Dev {
    unsafe { (&mut *core::ptr::addr_of_mut!(NETDEV)).as_mut().unwrap() }
}

// ---------------- smoltcp Device wrapper ----------------

struct SmolDevice;

struct RxToken(Vec<u8>);
struct TxToken;

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R {
        f(&mut self.0)
    }
}

impl phy::TxToken for TxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let dev = netdev();
        let mut tx = dev.new_tx_buffer(len);
        let r = f(tx.packet_mut());
        dev.send(tx).expect("virtio-net send failed");
        r
    }
}

impl Device for SmolDevice {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken;

    fn receive(&mut self, _t: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let dev = netdev();
        if !dev.can_recv() {
            return None;
        }
        match dev.receive() {
            Ok(buf) => {
                let packet = buf.packet().to_vec();
                dev.recycle_rx_buffer(buf).ok();
                Some((RxToken(packet), TxToken))
            }
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _t: Instant) -> Option<Self::TxToken<'_>> {
        if netdev().can_send() {
            Some(TxToken)
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ethernet;
        caps
    }
}

// ---------------- Interface + sockets ----------------

pub struct NetInner {
    iface: Interface,
    sockets: SocketSet<'static>,
}

static mut NET: Option<NetInner> = None;

fn net() -> &'static mut NetInner {
    unsafe { (&mut *core::ptr::addr_of_mut!(NET)).as_mut().unwrap() }
}

const OUR_IP: Ipv4Address = Ipv4Address::new(10, 0, 2, 15);
const GATEWAY: Ipv4Address = Ipv4Address::new(10, 0, 2, 2);

pub fn init() {
    // Probe virtio-mmio slots for a network device.
    let mut transport = None;
    for i in 0..VIRTIO_COUNT {
        let base = VIRTIO0_BASE + i * VIRTIO_STRIDE;
        let header = NonNull::new(base as *mut VirtIOHeader).unwrap();
        match unsafe { MmioTransport::new(header) } {
            Ok(t) if t.device_type() == DeviceType::Network => {
                transport = Some(t);
                break;
            }
            _ => {}
        }
    }
    let transport = transport.expect("no virtio-net device found");
    let dev = VirtIONet::<HalImpl, MmioTransport, QS>::new(transport, NET_BUF_LEN)
        .expect("failed to init virtio-net");
    let mac = dev.mac_address();
    unsafe {
        NETDEV = Some(dev);
    }
    crate::println!("[net] virtio-net mac = {:02x?}", mac);

    let mut device = SmolDevice;
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let now = Instant::from_millis(time::now_ms() as i64);
    let mut iface = Interface::new(config, &mut device, now);
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::Ipv4(OUR_IP), 24))
            .unwrap();
    });
    iface.routes_mut().add_default_ipv4_route(GATEWAY).unwrap();

    let sockets = SocketSet::new(vec![]);
    unsafe {
        NET = Some(NetInner { iface, sockets });
    }
    crate::println!("[net] interface up: {} gw {}", OUR_IP, GATEWAY);
}

/// Advance the TCP/IP stack: pump the device and process timers.
pub fn poll() {
    let now = Instant::from_millis(time::now_ms() as i64);
    let mut device = SmolDevice;
    let n = net();
    n.iface.poll(now, &mut device, &mut n.sockets);
}

// ---------------- socket table ----------------

struct SockEntry {
    listener: bool,
    port: u16,
    backlog: usize,
    pool: Vec<SocketHandle>,
    handle: Option<SocketHandle>,
    nonblock: bool,
}

static mut SOCKS: Option<Vec<Option<SockEntry>>> = None;

fn socks() -> &'static mut Vec<Option<SockEntry>> {
    unsafe {
        let s = &mut *core::ptr::addr_of_mut!(SOCKS);
        if s.is_none() {
            *s = Some(Vec::new());
        }
        s.as_mut().unwrap()
    }
}

fn new_tcp_socket() -> SocketHandle {
    let rx = tcp::SocketBuffer::new(vec![0u8; 32 * 1024]);
    let tx = tcp::SocketBuffer::new(vec![0u8; 32 * 1024]);
    let sock = tcp::Socket::new(rx, tx);
    net().sockets.add(sock)
}

fn tcp_ref(h: SocketHandle) -> &'static tcp::Socket<'static> {
    net().sockets.get::<tcp::Socket>(h)
}
fn tcp_mut(h: SocketHandle) -> &'static mut tcp::Socket<'static> {
    net().sockets.get_mut::<tcp::Socket>(h)
}

const EAGAIN: isize = -11;
const EBADF: isize = -9;
const ENOTCONN: isize = -107;
const EPIPE: isize = -32;
const EINVAL: isize = -22;

pub fn socket(nonblock: bool) -> usize {
    let entry = SockEntry {
        listener: false,
        port: 0,
        backlog: 0,
        pool: Vec::new(),
        handle: None,
        nonblock,
    };
    let table = socks();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(entry);
            return i;
        }
    }
    table.push(Some(entry));
    table.len() - 1
}

fn entry(idx: usize) -> Option<&'static mut SockEntry> {
    socks().get_mut(idx).and_then(|e| e.as_mut())
}

pub fn set_nonblock(idx: usize, nb: bool) {
    if let Some(e) = entry(idx) {
        e.nonblock = nb;
    }
}

pub fn is_nonblock(idx: usize) -> bool {
    entry(idx).map(|e| e.nonblock).unwrap_or(false)
}

pub fn bind(idx: usize, port: u16) -> isize {
    match entry(idx) {
        Some(e) => {
            e.port = port;
            0
        }
        None => EBADF,
    }
}

pub fn listen(idx: usize, backlog: usize) -> isize {
    let port = match entry(idx) {
        Some(e) => e.port,
        None => return EBADF,
    };
    let n = backlog.clamp(4, QS - 2);
    let mut pool = Vec::new();
    for _ in 0..n {
        let h = new_tcp_socket();
        if tcp_mut(h).listen(port).is_err() {
            return EINVAL;
        }
        pool.push(h);
    }
    let e = entry(idx).unwrap();
    e.listener = true;
    e.backlog = n;
    e.pool = pool;
    poll();
    0
}

fn conn_active(h: SocketHandle) -> bool {
    let s = tcp_ref(h);
    use tcp::State::*;
    !matches!(s.state(), Closed | Listen)
}

/// Try to accept a pending connection; returns Ok(new socket index) or Err(errno).
pub fn accept(idx: usize, nonblock: bool) -> Result<usize, isize> {
    loop {
        poll();
        let (found, port) = {
            let e = match entry(idx) {
                Some(e) if e.listener => e,
                Some(_) => return Err(EINVAL),
                None => return Err(EBADF),
            };
            let mut found = None;
            for (pos, &h) in e.pool.iter().enumerate() {
                if conn_active(h) {
                    found = Some((pos, h));
                    break;
                }
            }
            (found, e.port)
        };
        if let Some((pos, h)) = found {
            // Replace the consumed listening socket with a fresh one.
            let repl = new_tcp_socket();
            let _ = tcp_mut(repl).listen(port);
            let e = entry(idx).unwrap();
            e.pool[pos] = repl;
            // Create a stream socket entry for the accepted connection.
            let new_entry = SockEntry {
                listener: false,
                port,
                backlog: 0,
                pool: Vec::new(),
                handle: Some(h),
                nonblock,
            };
            let table = socks();
            for (i, slot) in table.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(new_entry);
                    return Ok(i);
                }
            }
            table.push(Some(new_entry));
            return Ok(table.len() - 1);
        }
        if nonblock {
            return Err(EAGAIN);
        }
        // Cooperative blocking: keep pumping the stack.
    }
}

pub fn recv(idx: usize, buf: &mut [u8]) -> isize {
    loop {
        poll();
        let (h, nb) = match entry(idx) {
            Some(e) => match e.handle {
                Some(h) => (h, e.nonblock),
                None => return ENOTCONN,
            },
            None => return EBADF,
        };
        let s = tcp_mut(h);
        if s.can_recv() {
            return s.recv_slice(buf).map(|n| n as isize).unwrap_or(0);
        }
        if !s.may_recv() {
            // Peer closed and no buffered data => EOF.
            return 0;
        }
        if nb {
            return EAGAIN;
        }
    }
}

pub fn send(idx: usize, buf: &[u8]) -> isize {
    if buf.is_empty() {
        return 0;
    }
    loop {
        poll();
        let (h, nb) = match entry(idx) {
            Some(e) => match e.handle {
                Some(h) => (h, e.nonblock),
                None => return ENOTCONN,
            },
            None => return EBADF,
        };
        let s = tcp_mut(h);
        if !s.may_send() {
            return EPIPE;
        }
        if s.can_send() {
            match s.send_slice(buf) {
                Ok(n) if n > 0 => {
                    poll();
                    return n as isize;
                }
                _ => {}
            }
        }
        if nb {
            return EAGAIN;
        }
    }
}

pub fn readable(idx: usize) -> bool {
    let e = match entry(idx) {
        Some(e) => e,
        None => return false,
    };
    if e.listener {
        e.pool.iter().any(|&h| conn_active(h))
    } else if let Some(h) = e.handle {
        let s = tcp_ref(h);
        s.can_recv() || !s.may_recv()
    } else {
        false
    }
}

pub fn writable(idx: usize) -> bool {
    let e = match entry(idx) {
        Some(e) => e,
        None => return false,
    };
    if let Some(h) = e.handle {
        let s = tcp_ref(h);
        s.may_send() && s.can_send()
    } else {
        false
    }
}

pub fn close(idx: usize) {
    if let Some(e) = entry(idx) {
        if let Some(h) = e.handle {
            tcp_mut(h).close();
        }
        for &h in &e.pool {
            tcp_mut(h).abort();
        }
        poll();
    }
    if let Some(slot) = socks().get_mut(idx) {
        *slot = None;
    }
}

pub fn local_ip_port(idx: usize) -> (u32, u16) {
    let ip = u32::from_be_bytes(OUR_IP.0);
    let port = entry(idx).map(|e| e.port).unwrap_or(0);
    (ip, port)
}
