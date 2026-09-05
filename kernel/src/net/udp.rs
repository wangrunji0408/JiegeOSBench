//! Minimal UDP sockets on smoltcp.
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

use super::socket::{i32_opt, Ancillary, SockAddr, SocketOps};
use super::{NET_WQ, STACK};
use crate::abi::*;
use crate::fs::file::{File, FileOps};
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

struct Inner {
    handle: Option<SocketHandle>,
    peer: Option<IpEndpoint>,
}

pub struct UdpSocket {
    inner: SpinLock<Inner>,
}

impl UdpSocket {
    pub fn new() -> Arc<UdpSocket> {
        Arc::new(UdpSocket { inner: SpinLock::new(Inner { handle: None, peer: None }) })
    }

    fn ensure_bound(&self, inner: &mut Inner, ep: IpListenEndpoint) -> Result<SocketHandle, i32> {
        if let Some(h) = inner.handle {
            return Ok(h);
        }
        let rx = udp::PacketBuffer::new(alloc::vec![udp::PacketMetadata::EMPTY; 16], alloc::vec![0u8; 64 * 1024]);
        let tx = udp::PacketBuffer::new(alloc::vec![udp::PacketMetadata::EMPTY; 16], alloc::vec![0u8; 64 * 1024]);
        let mut s = udp::Socket::new(rx, tx);
        let ep = if ep.port == 0 {
            IpListenEndpoint { addr: ep.addr, port: super::tcp::alloc_ephemeral_port() }
        } else {
            ep
        };
        s.bind(ep).map_err(|_| EADDRINUSE)?;
        let h = STACK.get().lock().sockets.add(s);
        inner.handle = Some(h);
        Ok(h)
    }
}

fn to_ep(addr: SockAddr) -> Result<IpEndpoint, i32> {
    match addr {
        SockAddr::Inet { addr, port } => Ok(IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from_octets(addr)), port)),
        _ => Err(EINVAL),
    }
}

fn from_ep(ep: IpEndpoint) -> SockAddr {
    match ep.addr {
        IpAddress::Ipv4(a) => SockAddr::Inet { addr: a.octets(), port: ep.port },
        #[allow(unreachable_patterns)]
        _ => SockAddr::Inet { addr: [0; 4], port: ep.port },
    }
}

impl SocketOps for UdpSocket {
    fn bind(&self, addr: SockAddr) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = addr else { return Err(EINVAL) };
        let mut inner = self.inner.lock();
        if inner.handle.is_some() {
            return Err(EINVAL);
        }
        let ip = if addr == [0, 0, 0, 0] { None } else { Some(IpAddress::Ipv4(Ipv4Address::from_octets(addr))) };
        self.ensure_bound(&mut inner, IpListenEndpoint { addr: ip, port })?;
        Ok(())
    }

    fn connect(&self, addr: SockAddr, _nonblock: bool) -> Result<(), i32> {
        let ep = to_ep(addr)?;
        let mut inner = self.inner.lock();
        self.ensure_bound(&mut inner, IpListenEndpoint { addr: None, port: 0 })?;
        inner.peer = Some(ep);
        Ok(())
    }

    fn send(&self, buf: &[u8], flags: u32, nonblock: bool, to: Option<SockAddr>, _anc: Ancillary) -> SysResult {
        let mut inner = self.inner.lock();
        let h = self.ensure_bound(&mut inner, IpListenEndpoint { addr: None, port: 0 })?;
        let dst = match to {
            Some(a) => to_ep(a)?,
            None => inner.peer.ok_or(EDESTADDRREQ)?,
        };
        drop(inner);
        let r = block_on(&[&NET_WQ], nonblock || flags & MSG_DONTWAIT != 0, || {
            let mut stack = STACK.get().lock();
            let s = stack.sockets.get_mut::<udp::Socket>(h);
            match s.send_slice(buf, dst) {
                Ok(()) => Ok(buf.len()),
                Err(udp::SendError::BufferFull) => Err(EAGAIN),
                Err(udp::SendError::Unaddressable) => Err(EHOSTUNREACH),
            }
        });
        super::poll();
        r
    }

    fn recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> Result<(usize, Option<SockAddr>, Ancillary), i32> {
        let h = self.inner.lock().handle.ok_or(ENOTCONN)?;
        block_on(&[&NET_WQ], nonblock || flags & MSG_DONTWAIT != 0, || {
            super::poll();
            let mut stack = STACK.get().lock();
            let s = stack.sockets.get_mut::<udp::Socket>(h);
            let r = if flags & MSG_PEEK != 0 {
                s.peek_slice(buf).map(|(n, m)| (n, m.endpoint))
            } else {
                s.recv_slice(buf).map(|(n, m)| (n, m.endpoint))
            };
            match r {
                Ok((n, ep)) => Ok((n, Some(from_ep(ep)), Ancillary::default())),
                Err(_) => Err(EAGAIN),
            }
        })
    }

    fn local_addr(&self) -> Result<SockAddr, i32> {
        let inner = self.inner.lock();
        match inner.handle {
            Some(h) => {
                let stack = STACK.get().lock();
                let ep = stack.sockets.get::<udp::Socket>(h).endpoint();
                Ok(SockAddr::Inet {
                    addr: match ep.addr {
                        Some(IpAddress::Ipv4(a)) => a.octets(),
                        _ => [0; 4],
                    },
                    port: ep.port,
                })
            }
            None => Ok(SockAddr::Inet { addr: [0; 4], port: 0 }),
        }
    }

    fn peer_addr(&self) -> Result<SockAddr, i32> {
        self.inner.lock().peer.map(from_ep).ok_or(ENOTCONN)
    }

    fn getsockopt(&self, level: i32, opt: i32) -> Result<Vec<u8>, i32> {
        match (level, opt) {
            (SOL_SOCKET, SO_TYPE) => Ok(i32_opt(SOCK_DGRAM as i32)),
            (SOL_SOCKET, SO_ERROR) => Ok(i32_opt(0)),
            (SOL_SOCKET, SO_DOMAIN) => Ok(i32_opt(AF_INET as i32)),
            (SOL_SOCKET, SO_PROTOCOL) => Ok(i32_opt(17)),
            (SOL_SOCKET, SO_RCVBUF) | (SOL_SOCKET, SO_SNDBUF) => Ok(i32_opt(65536)),
            _ => Err(ENOPROTOOPT),
        }
    }

    fn sock_type(&self) -> u32 {
        SOCK_DGRAM
    }

    fn domain(&self) -> u16 {
        AF_INET
    }
}

impl FileOps for UdpSocket {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        self.recv(buf, 0, file.nonblock()).map(|(n, _, _)| n)
    }

    fn write_at(&self, _off: u64, buf: &[u8], file: &File) -> SysResult {
        self.send(buf, 0, file.nonblock(), None, Ancillary::default())
    }

    fn poll(&self) -> u32 {
        let inner = self.inner.lock();
        let mut ev = POLLOUT;
        if let Some(h) = inner.handle {
            let stack = STACK.get().lock();
            if stack.sockets.get::<udp::Socket>(h).can_recv() {
                ev |= POLLIN;
            }
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&NET_WQ)
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(Stat { st_mode: S_IFSOCK | 0o777, st_nlink: 1, st_blksize: 4096, ..Stat::default() })
    }

    fn as_socket(&self) -> Option<&dyn SocketOps> {
        Some(self)
    }

    fn release(&self) {
        if let Some(h) = self.inner.lock().handle.take() {
            STACK.get().lock().sockets.remove(h);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
