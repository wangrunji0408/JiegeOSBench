/// Socket管理
/// 提供类Linux的socket API给syscall层

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use lazy_static::lazy_static;

pub use smoltcp::iface::SocketHandle as SmoltcpSocketHandle;

/// 内核中的socket状态
pub enum KernelSocket {
    Tcp {
        handle: SmoltcpSocketHandle,
        local_addr: Option<smoltcp::wire::IpEndpoint>,
        remote_addr: Option<smoltcp::wire::IpEndpoint>,
        is_listener: bool,
    },
    Udp {
        handle: SmoltcpSocketHandle,
        local_addr: Option<smoltcp::wire::IpEndpoint>,
    },
}

pub type SocketHandle = i32;

lazy_static! {
    pub static ref SOCKETS: Mutex<BTreeMap<i32, Arc<Mutex<KernelSocket>>>> =
        Mutex::new(BTreeMap::new());
    static ref NEXT_SOCK_FD: Mutex<i32> = Mutex::new(1000);
}

pub fn init() {}

pub fn get_socket_by_fd(fd: i32) -> Option<Arc<Mutex<KernelSocket>>> {
    SOCKETS.lock().get(&fd).cloned()
}

pub fn alloc_socket_fd(sock: KernelSocket) -> i32 {
    let mut next = NEXT_SOCK_FD.lock();
    let fd = *next;
    *next += 1;
    SOCKETS.lock().insert(fd, Arc::new(Mutex::new(sock)));
    fd
}

pub fn remove_socket(fd: i32) {
    SOCKETS.lock().remove(&fd);
}

/// 创建TCP socket buffer（使用静态分配）
pub fn create_tcp_socket() -> i32 {
    let rx_buf = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0u8; 65536]);
    let tx_buf = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0u8; 65536]);
    let tcp_socket = smoltcp::socket::tcp::Socket::new(rx_buf, tx_buf);

    let handle = {
        let mut guard = super::NETWORK.lock();
        if let Some(state) = guard.as_mut() {
            state.sockets.add(tcp_socket)
        } else {
            return -1;
        }
    };

    alloc_socket_fd(KernelSocket::Tcp {
        handle,
        local_addr: None,
        remote_addr: None,
        is_listener: false,
    })
}

pub fn create_udp_socket() -> i32 {
    let rx_meta = alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 64];
    let tx_meta = alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 64];
    let rx_data = alloc::vec![0u8; 65536];
    let tx_data = alloc::vec![0u8; 65536];

    let rx_buf = smoltcp::socket::udp::PacketBuffer::new(rx_meta, rx_data);
    let tx_buf = smoltcp::socket::udp::PacketBuffer::new(tx_meta, tx_data);
    let udp_socket = smoltcp::socket::udp::Socket::new(rx_buf, tx_buf);

    let handle = {
        let mut guard = super::NETWORK.lock();
        if let Some(state) = guard.as_mut() {
            state.sockets.add(udp_socket)
        } else {
            return -1;
        }
    };

    alloc_socket_fd(KernelSocket::Udp {
        handle,
        local_addr: None,
    })
}

pub fn poll_sockets(_sockets: &mut smoltcp::iface::SocketSet<'static>) {
    crate::task::wake_io_tasks();
}
