use alloc::collections::BTreeMap;
use alloc::vec;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, IpListenEndpoint, Ipv4Address,
};
use spin::Mutex;

use crate::drivers::virtio_net::VirtioNet;

const EBADF: i32 = 9;
const EAGAIN: i32 = 11;
const EINVAL: i32 = 22;
const EPIPE: i32 = 32;
const ENOSYS: i32 = 38;
const ENETDOWN: i32 = 100;
const EADDRINUSE: i32 = 98;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;

#[derive(Clone, Copy)]
struct Binding {
    address: Option<IpAddress>,
    port: u16,
}

impl Binding {
    fn endpoint(self) -> IpListenEndpoint {
        IpListenEndpoint {
            addr: self.address,
            port: self.port,
        }
    }
}

struct SocketRecord {
    handle: smoltcp::iface::SocketHandle,
    binding: Option<Binding>,
    listening: bool,
}

struct Network {
    device: VirtioNet,
    interface: Interface,
    sockets: SocketSet<'static>,
    records: BTreeMap<usize, SocketRecord>,
    next_id: usize,
}

static NETWORK: Mutex<Option<Network>> = Mutex::new(None);

type NetResult<T> = Result<T, i32>;

fn instant() -> Instant {
    Instant::from_millis((crate::arch::time() / 10_000) as i64)
}

fn with_network<R>(function: impl FnOnce(&mut Network) -> R) -> NetResult<R> {
    let mut guard = NETWORK.lock();
    let network = guard.as_mut().ok_or(ENETDOWN)?;
    Ok(function(network))
}

fn allocate_tcp(network: &mut Network) -> smoltcp::iface::SocketHandle {
    let receive = tcp::SocketBuffer::new(vec![0; 32 * 1024]);
    let transmit = tcp::SocketBuffer::new(vec![0; 32 * 1024]);
    network.sockets.add(tcp::Socket::new(receive, transmit))
}

fn poll_network(network: &mut Network) {
    let Network {
        device,
        interface,
        sockets,
        ..
    } = network;
    interface.poll(instant(), device, sockets);
}

pub fn init() {
    for (index, base) in (0x1000_1000..=0x1000_8000)
        .step_by(0x1000)
        .enumerate()
    {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        match VirtioNet::new(base, mac) {
            Ok(mut device) => {
                let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
                config.random_seed = crate::arch::time();
                let mut interface = Interface::new(config, &mut device, instant());
                interface.update_ip_addrs(|addresses| {
                    addresses
                        .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
                        .unwrap()
                });
                interface
                    .routes_mut()
                    .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
                    .ok();
                *NETWORK.lock() = Some(Network {
                    device,
                    interface,
                    sockets: SocketSet::new(vec![]),
                    records: BTreeMap::new(),
                    next_id: 1,
                });
                crate::println!(
                    "net: virtio-mmio slot {} irq={} address=10.0.2.15/24 gateway=10.0.2.2",
                    index + 1,
                    index + 1
                );
                return;
            }
            Err(_) => {}
        }
    }
    crate::println!("net: no modern virtio-net device found");
}

pub fn poll() {
    if let Some(network) = NETWORK.lock().as_mut() {
        poll_network(network);
    }
}

pub fn interrupt() {
    if let Some(network) = NETWORK.lock().as_mut() {
        network.device.interrupt_ack();
        poll_network(network);
    }
}

pub fn create() -> NetResult<usize> {
    with_network(|network| {
        let handle = allocate_tcp(network);
        let id = network.next_id;
        network.next_id += 1;
        network.records.insert(
            id,
            SocketRecord {
                handle,
                binding: None,
                listening: false,
            },
        );
        id
    })
}

pub fn close(id: usize) {
    if let Some(network) = NETWORK.lock().as_mut() {
        if let Some(record) = network.records.remove(&id) {
            network.sockets.remove(record.handle);
        }
    }
}

pub fn bind(id: usize, address: [u8; 4], port: u16) -> NetResult<()> {
    with_network(|network| {
        if network.records.values().any(|record| {
            record.binding.is_some_and(|binding| {
                binding.port == port && id != 0 && !core::ptr::eq(record, network.records.get(&id).unwrap())
            })
        }) {
            return Err(EADDRINUSE);
        }
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        let address = if address == [0, 0, 0, 0] {
            None
        } else {
            Some(IpAddress::v4(address[0], address[1], address[2], address[3]))
        };
        record.binding = Some(Binding { address, port });
        Ok(())
    })?
}

pub fn listen(id: usize) -> NetResult<()> {
    let result = with_network(|network| {
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        let binding = record.binding.ok_or(EINVAL)?;
        let socket = network.sockets.get_mut::<tcp::Socket>(record.handle);
        socket.listen(binding.endpoint()).map_err(|_| EINVAL)?;
        record.listening = true;
        Ok(())
    })?;
    poll();
    result
}

fn accept_ready(socket: &tcp::Socket) -> bool {
    matches!(socket.state(), tcp::State::Established | tcp::State::CloseWait)
        || socket.recv_queue() != 0
}

pub fn accept(id: usize) -> NetResult<(usize, Option<IpEndpoint>)> {
    let accepted = with_network(|network| {
        let (old_handle, binding) = {
            let record = network.records.get(&id).ok_or(EBADF)?;
            if !record.listening {
                return Err(EINVAL);
            }
            let socket = network.sockets.get::<tcp::Socket>(record.handle);
            if !accept_ready(socket) {
                return Err(EAGAIN);
            }
            (record.handle, record.binding.ok_or(EINVAL)?)
        };
        let replacement = allocate_tcp(network);
        network
            .sockets
            .get_mut::<tcp::Socket>(replacement)
            .listen(binding.endpoint())
            .map_err(|_| EINVAL)?;
        let remote = network
            .sockets
            .get::<tcp::Socket>(old_handle)
            .remote_endpoint();
        let new_id = network.next_id;
        network.next_id += 1;
        network.records.get_mut(&id).unwrap().handle = replacement;
        network.records.insert(
            new_id,
            SocketRecord {
                handle: old_handle,
                binding: Some(binding),
                listening: false,
            },
        );
        Ok((new_id, remote))
    })?;
    poll();
    accepted
}

pub fn receive(id: usize, buffer: &mut [u8]) -> NetResult<usize> {
    poll();
    with_network(|network| {
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        if record.listening {
            return Err(EINVAL);
        }
        let socket = network.sockets.get_mut::<tcp::Socket>(record.handle);
        if socket.recv_queue() == 0 {
            return if socket.may_recv() { Err(EAGAIN) } else { Ok(0) };
        }
        socket.recv_slice(buffer).map_err(|_| EAGAIN)
    })?
}

pub fn send(id: usize, buffer: &[u8]) -> NetResult<usize> {
    poll();
    let result = with_network(|network| {
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        if record.listening {
            return Err(EINVAL);
        }
        let socket = network.sockets.get_mut::<tcp::Socket>(record.handle);
        if !socket.may_send() {
            return Err(EPIPE);
        }
        match socket.send_slice(buffer) {
            Ok(0) => Err(EAGAIN),
            Ok(count) => Ok(count),
            Err(_) => Err(EAGAIN),
        }
    })?;
    poll();
    result
}

pub fn shutdown(id: usize) -> NetResult<()> {
    let result = with_network(|network| {
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        network.sockets.get_mut::<tcp::Socket>(record.handle).close();
        Ok(())
    })?;
    poll();
    result
}

pub fn disable_nagle(id: usize) -> NetResult<()> {
    with_network(|network| {
        let record = network.records.get_mut(&id).ok_or(EBADF)?;
        network
            .sockets
            .get_mut::<tcp::Socket>(record.handle)
            .set_nagle_enabled(false);
        Ok(())
    })?
}

pub fn event_flags(id: usize) -> u32 {
    let Ok(events) = with_network(|network| {
        let Some(record) = network.records.get(&id) else {
            return EPOLLERR | EPOLLHUP;
        };
        let socket = network.sockets.get::<tcp::Socket>(record.handle);
        if record.listening {
            return if accept_ready(socket) { EPOLLIN } else { 0 };
        }
        let mut events = 0;
        if socket.recv_queue() != 0 {
            events |= EPOLLIN;
        }
        if !socket.may_recv() {
            events |= EPOLLIN | EPOLLRDHUP | EPOLLHUP;
        }
        if socket.may_send() {
            events |= EPOLLOUT;
        }
        if socket.state() == tcp::State::Closed {
            events |= EPOLLERR | EPOLLHUP;
        }
        events
    }) else {
        return EPOLLERR;
    };
    events
}

pub fn local_endpoint(id: usize) -> NetResult<Option<IpEndpoint>> {
    with_network(|network| {
        let record = network.records.get(&id).ok_or(EBADF)?;
        let socket = network.sockets.get::<tcp::Socket>(record.handle);
        if let Some(endpoint) = socket.local_endpoint() {
            return Ok(Some(endpoint));
        }
        Ok(record.binding.map(|binding| IpEndpoint {
            addr: binding.address.unwrap_or(IpAddress::v4(0, 0, 0, 0)),
            port: binding.port,
        }))
    })?
}

pub fn remote_endpoint(id: usize) -> NetResult<Option<IpEndpoint>> {
    with_network(|network| {
        let record = network.records.get(&id).ok_or(EBADF)?;
        Ok(network
            .sockets
            .get::<tcp::Socket>(record.handle)
            .remote_endpoint())
    })?
}

pub fn connect(_id: usize, _address: [u8; 4], _port: u16) -> NetResult<()> {
    Err(ENOSYS)
}
