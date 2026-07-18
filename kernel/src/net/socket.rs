//! `File`-trait wrappers around smoltcp TCP sockets, so they can live in
//! the regular fd table alongside tmpfs files and stdio.

use super::listener::Listener;
use crate::fs::File;
use core::sync::atomic::{AtomicBool, Ordering};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint};

pub struct TcpStreamFile {
    pub handle: SocketHandle,
    nonblocking: AtomicBool,
}

impl TcpStreamFile {
    pub fn new(handle: SocketHandle) -> Self {
        Self {
            handle,
            nonblocking: AtomicBool::new(false),
        }
    }

    pub fn local_endpoint(&self) -> Option<IpEndpoint> {
        super::with_net(|state| state.sockets.get::<tcp::Socket>(self.handle).local_endpoint())?
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        super::with_net(|state| state.sockets.get::<tcp::Socket>(self.handle).remote_endpoint())?
    }
}

impl File for TcpStreamFile {
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        super::poll();
        super::with_net(|state| {
            state
                .sockets
                .get_mut::<tcp::Socket>(self.handle)
                .recv_slice(buf)
                .unwrap_or(0)
        })
        .unwrap_or(0)
    }
    fn write(&self, buf: &[u8]) -> usize {
        let n = super::with_net(|state| {
            state
                .sockets
                .get_mut::<tcp::Socket>(self.handle)
                .send_slice(buf)
                .unwrap_or(0)
        })
        .unwrap_or(0);
        super::poll();
        n
    }
    fn poll_readable(&self) -> bool {
        super::poll();
        super::with_net(|state| {
            let socket = state.sockets.get::<tcp::Socket>(self.handle);
            socket.can_recv() || !socket.may_recv()
        })
        .unwrap_or(true)
    }
    fn poll_writable(&self) -> bool {
        super::with_net(|state| state.sockets.get::<tcp::Socket>(self.handle).can_send()).unwrap_or(true)
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

impl Drop for TcpStreamFile {
    fn drop(&mut self) {
        super::with_net(|state| {
            state.sockets.get_mut::<tcp::Socket>(self.handle).abort();
            state.sockets.remove(self.handle);
        });
    }
}

pub struct TcpListenerFile {
    pub port: u16,
    nonblocking: AtomicBool,
}

impl TcpListenerFile {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            nonblocking: AtomicBool::new(false),
        }
    }

    pub fn accept(&self) -> Option<SocketHandle> {
        super::poll();
        super::with_net(|state| {
            state
                .listeners
                .iter_mut()
                .find(|l| l.port == self.port)
                .and_then(|l: &mut Listener| l.accept_queue.pop_front())
        })
        .flatten()
    }
}

impl File for TcpListenerFile {
    fn readable(&self) -> bool {
        true
    }
    fn poll_readable(&self) -> bool {
        super::poll();
        super::with_net(|state| {
            state
                .listeners
                .iter()
                .find(|l| l.port == self.port)
                .map(|l| !l.accept_queue.is_empty())
        })
        .flatten()
        .unwrap_or(false)
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

pub fn ip_from_be_u32(addr: u32) -> IpAddress {
    IpAddress::v4(
        (addr >> 24) as u8,
        (addr >> 16) as u8,
        (addr >> 8) as u8,
        addr as u8,
    )
}
