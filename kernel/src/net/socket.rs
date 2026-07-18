//! `File`-trait wrapper around a TCP socket that can transition through
//! the states a POSIX socket fd goes through: created, bound, then either
//! listening or connected.

use crate::fs::File;
use core::sync::atomic::{AtomicBool, Ordering};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpEndpoint, IpListenEndpoint};
use spin::Mutex;

enum Kind {
    Unbound,
    Listening(u16),
    Stream(SocketHandle),
}

pub struct TcpFile {
    kind: Mutex<Kind>,
    bound_port: Mutex<Option<u16>>,
    nonblocking: AtomicBool,
}

impl TcpFile {
    pub fn new() -> Self {
        Self {
            kind: Mutex::new(Kind::Unbound),
            bound_port: Mutex::new(None),
            nonblocking: AtomicBool::new(false),
        }
    }

    pub fn bind(&self, port: u16) {
        *self.bound_port.lock() = Some(port);
    }

    pub fn bound_port(&self) -> Option<u16> {
        *self.bound_port.lock()
    }

    /// Start listening on the previously-`bind`ed port (or an ephemeral
    /// one if never bound).
    pub fn listen(&self) -> Result<(), ()> {
        let port = self.bound_port().unwrap_or(0);
        super::with_net(|state| {
            if !state.listeners.iter().any(|l| l.port == port) {
                let listener = super::listener::Listener::new(&mut state.sockets, port);
                state.listeners.push(listener);
            }
        });
        *self.kind.lock() = Kind::Listening(port);
        Ok(())
    }

    pub fn accept(&self) -> Option<SocketHandle> {
        let port = match &*self.kind.lock() {
            Kind::Listening(p) => *p,
            _ => return None,
        };
        super::poll();
        super::with_net(|state| {
            state
                .listeners
                .iter_mut()
                .find(|l| l.port == port)
                .and_then(|l| l.accept_queue.pop_front())
        })
        .flatten()
    }

    pub fn connect(&self, remote: IpEndpoint) -> Result<(), ()> {
        let handle = super::with_net(|state| {
            let rx = tcp::SocketBuffer::new(alloc::vec![0u8; 128 * 1024]);
            let tx = tcp::SocketBuffer::new(alloc::vec![0u8; 128 * 1024]);
            let mut socket = tcp::Socket::new(rx, tx);
            let local_port = self.bound_port().unwrap_or(49152 + (remote.port % 10000));
            socket
                .connect(state.iface.context(), remote, local_port)
                .map_err(|_| ())?;
            Ok(state.sockets.add(socket))
        })
        .ok_or(())??;
        *self.kind.lock() = Kind::Stream(handle);
        Ok(())
    }

    pub fn local_endpoint(&self) -> Option<IpEndpoint> {
        match &*self.kind.lock() {
            Kind::Stream(h) => super::with_net(|state| state.sockets.get::<tcp::Socket>(*h).local_endpoint())?,
            _ => None,
        }
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        match &*self.kind.lock() {
            Kind::Stream(h) => super::with_net(|state| state.sockets.get::<tcp::Socket>(*h).remote_endpoint())?,
            _ => None,
        }
    }

    pub fn from_accepted(handle: SocketHandle) -> Self {
        Self {
            kind: Mutex::new(Kind::Stream(handle)),
            bound_port: Mutex::new(None),
            nonblocking: AtomicBool::new(false),
        }
    }

    pub fn shutdown(&self) {
        if let Kind::Stream(h) = &*self.kind.lock() {
            super::with_net(|state| state.sockets.get_mut::<tcp::Socket>(*h).close());
        }
    }
}

impl File for TcpFile {
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        super::poll();
        let h = match &*self.kind.lock() {
            Kind::Stream(h) => *h,
            _ => return 0,
        };
        super::with_net(|state| state.sockets.get_mut::<tcp::Socket>(h).recv_slice(buf).unwrap_or(0)).unwrap_or(0)
    }
    fn write(&self, buf: &[u8]) -> usize {
        let h = match &*self.kind.lock() {
            Kind::Stream(h) => *h,
            _ => return 0,
        };
        let n =
            super::with_net(|state| state.sockets.get_mut::<tcp::Socket>(h).send_slice(buf).unwrap_or(0))
                .unwrap_or(0);
        super::poll();
        n
    }
    fn poll_readable(&self) -> bool {
        super::poll();
        match &*self.kind.lock() {
            Kind::Stream(h) => super::with_net(|state| {
                let socket = state.sockets.get::<tcp::Socket>(*h);
                socket.can_recv() || !socket.may_recv()
            })
            .unwrap_or(true),
            Kind::Listening(port) => super::with_net(|state| {
                state
                    .listeners
                    .iter()
                    .find(|l| l.port == *port)
                    .map(|l| !l.accept_queue.is_empty())
            })
            .flatten()
            .unwrap_or(false),
            Kind::Unbound => true,
        }
    }
    fn poll_writable(&self) -> bool {
        match &*self.kind.lock() {
            Kind::Stream(h) => super::with_net(|state| state.sockets.get::<tcp::Socket>(*h).can_send()).unwrap_or(true),
            _ => true,
        }
    }
    fn is_nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Relaxed)
    }
    fn set_nonblocking(&self, v: bool) {
        self.nonblocking.store(v, Ordering::Relaxed);
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl Drop for TcpFile {
    fn drop(&mut self) {
        if let Kind::Stream(h) = &*self.kind.lock() {
            super::with_net(|state| {
                state.sockets.get_mut::<tcp::Socket>(*h).abort();
                state.sockets.remove(*h);
            });
        }
    }
}

pub fn accepted_endpoint(handle: SocketHandle) -> Option<IpEndpoint> {
    super::with_net(|state| state.sockets.get::<tcp::Socket>(handle).remote_endpoint())?
}

/// `IpListenEndpoint` for "any address" on `port`, as used by `listen()`.
pub fn any_endpoint(port: u16) -> IpListenEndpoint {
    IpListenEndpoint { addr: None, port }
}
