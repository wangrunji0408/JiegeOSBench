use super::NetState;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::IpListenEndpoint;

const BACKLOG_TARGET: usize = 4;
const SOCKET_BUFFER_SIZE: usize = 128 * 1024;

pub struct Listener {
    pub port: u16,
    backlog: Vec<SocketHandle>,
    pub accept_queue: VecDeque<SocketHandle>,
}

fn new_listening_socket(sockets: &mut smoltcp::iface::SocketSet<'static>, port: u16) -> SocketHandle {
    let rx = tcp::SocketBuffer::new(alloc::vec![0u8; SOCKET_BUFFER_SIZE]);
    let tx = tcp::SocketBuffer::new(alloc::vec![0u8; SOCKET_BUFFER_SIZE]);
    let mut socket = tcp::Socket::new(rx, tx);
    socket
        .listen(IpListenEndpoint { addr: None, port })
        .expect("listen failed");
    sockets.add(socket)
}

impl Listener {
    pub fn new(sockets: &mut smoltcp::iface::SocketSet<'static>, port: u16) -> Self {
        let mut backlog = Vec::new();
        for _ in 0..BACKLOG_TARGET {
            backlog.push(new_listening_socket(sockets, port));
        }
        Self {
            port,
            backlog,
            accept_queue: VecDeque::new(),
        }
    }
}

/// Move any backlog socket that has left the `Listen` state into the
/// accept queue, and top the backlog back up so new connections keep
/// being accepted while we do.
pub fn service_listeners(state: &mut NetState) {
    for listener in state.listeners.iter_mut() {
        let mut i = 0;
        while i < listener.backlog.len() {
            let handle = listener.backlog[i];
            let is_listening = state.sockets.get::<tcp::Socket>(handle).is_listening();
            if !is_listening {
                listener.backlog.remove(i);
                listener.accept_queue.push_back(handle);
            } else {
                i += 1;
            }
        }
        while listener.backlog.len() < BACKLOG_TARGET {
            listener
                .backlog
                .push(new_listening_socket(&mut state.sockets, listener.port));
        }
    }
}
