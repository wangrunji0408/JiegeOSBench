//! Sockets as VFS inodes.
//!
//! smoltcp has no listen backlog: a `tcp::Socket` in `Listen` state becomes the
//! connection when a SYN arrives. To give nginx a POSIX listening socket that
//! can be accepted from repeatedly, a listening socket owns a pool of smoltcp
//! sockets all listening on the same endpoint. `accept` takes one that has
//! become connected and replaces it with a fresh listener, so the backlog stays
//! filled.

use super::addr::SockAddr;
use super::stack;
use crate::fs::inode::{next_ino, Inode, InodeKind};
use crate::fs::{self, Result};
use crate::{bail, impl_as_any};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::IpListenEndpoint;
use spin::Mutex;

/// Socket types.
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;
/// Flags OR-ed into the type argument of `socket()`.
pub const SOCK_NONBLOCK: u32 = 0o4000;
pub const SOCK_CLOEXEC: u32 = 0o2000;

/// `sendto` on an unconnected datagram socket with no destination.
const EDESTADDRREQ: isize = 89;

/// Default socket buffer sizes. nginx serves static files with `sendfile`-sized
/// writes, so a generous TX buffer keeps it from blocking.
const DEFAULT_RX_BUFFER: usize = 64 * 1024;
const DEFAULT_TX_BUFFER: usize = 64 * 1024;
/// How many smoltcp sockets a listener keeps parked on its endpoint.
const LISTEN_POOL: usize = 16;

/// What kind of socket this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SocketKind {
    Tcp,
    Udp,
    /// A socket family we accept but cannot carry traffic for (`AF_UNIX`,
    /// `AF_NETLINK`). Operations mostly succeed vacuously so that programs
    /// probing for them don't fail outright.
    Other,
}

/// The connection state of a socket, mirroring what the syscall layer needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SockState {
    Unbound,
    Bound,
    Listening,
    Connecting,
    Connected,
    Closed,
}

struct Inner {
    /// The smoltcp handle for a connected or connecting socket.
    handle: Option<SocketHandle>,
    /// For a listener: the pool of parked sockets.
    pool: Vec<SocketHandle>,
    /// The endpoint we bound to.
    local: Option<SockAddr>,
    /// The peer, once connected.
    peer: Option<SockAddr>,
    state: SockState,
    /// The listen endpoint, so we can replenish the pool.
    listen_endpoint: Option<IpListenEndpoint>,
    /// Backlog requested by `listen`.
    backlog: usize,
    /// This socket accepts operations but carries no traffic — an IPv6 listener
    /// on a stack that only speaks IPv4.
    inert: bool,
}

pub struct Socket {
    ino: u64,
    pub kind: SocketKind,
    pub family: u16,
    inner: Mutex<Inner>,
    /// `SOCK_NONBLOCK` / `O_NONBLOCK`.
    pub nonblock: AtomicBool,
    /// Socket options we remember so `getsockopt` echoes them back.
    pub reuseaddr: AtomicBool,
    pub reuseport: AtomicBool,
    pub keepalive: AtomicBool,
    pub nodelay: AtomicBool,
    pub sndbuf: AtomicUsize,
    pub rcvbuf: AtomicUsize,
    /// Pending error to report from `getsockopt(SO_ERROR)`.
    pub error: AtomicI32,
    /// Receive/send timeouts in ms; 0 means none.
    pub rcvtimeo_ms: AtomicU32,
    pub sndtimeo_ms: AtomicU32,
    /// Shutdown state.
    pub shut_rd: AtomicBool,
    pub shut_wr: AtomicBool,
}

impl Socket {
    pub fn new(family: u16, kind: SocketKind, nonblock: bool) -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            kind,
            family,
            inner: Mutex::new(Inner {
                handle: None,
                pool: Vec::new(),
                local: None,
                peer: None,
                state: SockState::Unbound,
                listen_endpoint: None,
                backlog: 0,
                inert: false,
            }),
            nonblock: AtomicBool::new(nonblock),
            reuseaddr: AtomicBool::new(false),
            reuseport: AtomicBool::new(false),
            keepalive: AtomicBool::new(false),
            nodelay: AtomicBool::new(false),
            sndbuf: AtomicUsize::new(DEFAULT_TX_BUFFER),
            rcvbuf: AtomicUsize::new(DEFAULT_RX_BUFFER),
            error: AtomicI32::new(0),
            rcvtimeo_ms: AtomicU32::new(0),
            sndtimeo_ms: AtomicU32::new(0),
            shut_rd: AtomicBool::new(false),
            shut_wr: AtomicBool::new(false),
        })
    }

    /// Build a socket that already owns a connected smoltcp handle (the result
    /// of `accept`).
    fn from_accepted(family: u16, handle: SocketHandle, local: SockAddr, peer: SockAddr) -> Arc<Self> {
        let socket = Self::new(family, SocketKind::Tcp, false);
        {
            let mut inner = socket.inner.lock();
            inner.handle = Some(handle);
            inner.local = Some(local);
            inner.peer = Some(peer);
            inner.state = SockState::Connected;
        }
        socket
    }

    pub fn state(&self) -> SockState {
        self.inner.lock().state
    }

    pub fn is_nonblock(&self) -> bool {
        self.nonblock.load(Ordering::Relaxed)
    }

    fn timeout_deadline(&self, timeout_ms: u32) -> Option<u64> {
        if timeout_ms == 0 {
            None
        } else {
            Some(crate::time::monotonic_ms() + timeout_ms as u64)
        }
    }

    /// `bind`.
    pub fn bind(&self, addr: SockAddr) -> Result<()> {
        if self.kind == SocketKind::Other {
            // Accept the bind so `AF_UNIX` probes succeed.
            self.inner.lock().local = Some(addr);
            self.inner.lock().state = SockState::Bound;
            return Ok(());
        }

        // We carry no IPv6 traffic, so an AF_INET6 socket becomes inert: it binds
        // and listens successfully but never sees a connection. This mirrors a
        // real dual-stack kernel with IPV6_V6ONLY set, and matters because nginx
        // configures `listen [::]:80` alongside `listen 80` and treats a failure
        // on either as fatal — while two live listeners on one port would swallow
        // connections nobody ever accepts.
        if matches!(addr, SockAddr::V6 { .. }) {
            let mut inner = self.inner.lock();
            if inner.state != SockState::Unbound {
                bail!(EINVAL);
            }
            inner.local = Some(addr);
            inner.state = SockState::Bound;
            inner.inert = true;
            return Ok(());
        }

        let mut inner = self.inner.lock();
        if inner.state != SockState::Unbound {
            bail!(EINVAL);
        }
        let endpoint = addr.to_listen_endpoint()?;
        // Port 0 means "pick one".
        let endpoint = if endpoint.port == 0 {
            IpListenEndpoint {
                addr: endpoint.addr,
                port: stack::ephemeral_port(),
            }
        } else {
            // Claim the explicit port. SO_REUSEADDR lets a restarting server
            // rebind a port left over from a socket that has since closed, which
            // is exactly what nginx does on reload.
            if !stack::claim_port(endpoint.port) && !self.reuseaddr.load(Ordering::Relaxed) {
                bail!(EADDRINUSE);
            }
            endpoint
        };

        if self.kind == SocketKind::Udp {
            let handle = stack::add_udp_socket(self.rcvbuf.load(Ordering::Relaxed), self.sndbuf.load(Ordering::Relaxed))
                .ok_or(fs::Error::new(fs::errno::ENOBUFS))?;
            let bound = stack::with_udp(handle, |socket| socket.bind(endpoint))
                .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;
            if bound.is_err() {
                stack::remove_socket(handle);
                bail!(EADDRINUSE);
            }
            inner.handle = Some(handle);
        }

        inner.listen_endpoint = Some(endpoint);
        inner.local = Some(match endpoint.addr {
            Some(smoltcp::wire::IpAddress::Ipv4(v4)) => SockAddr::V4 {
                addr: Some(v4),
                port: endpoint.port,
            },
            None => SockAddr::V4 {
                addr: None,
                port: endpoint.port,
            },
        });
        inner.state = SockState::Bound;
        Ok(())
    }

    /// `listen`: fill the pool with sockets parked on our endpoint.
    pub fn listen(&self, backlog: usize) -> Result<()> {
        if self.kind != SocketKind::Tcp {
            bail!(EOPNOTSUPP);
        }
        let mut inner = self.inner.lock();
        if inner.state == SockState::Listening {
            // Linux allows re-listening to change the backlog.
            inner.backlog = backlog;
            return Ok(());
        }
        if inner.state != SockState::Bound {
            bail!(EINVAL);
        }
        // An inert (IPv6) listener needs no pool: report success and stay quiet.
        if inner.inert {
            inner.backlog = backlog;
            inner.state = SockState::Listening;
            return Ok(());
        }
        let endpoint = inner
            .listen_endpoint
            .ok_or(fs::Error::new(fs::errno::EINVAL))?;

        // Only one live listener per port. Two pools on the same endpoint would
        // both attract SYNs, and connections landing in the pool whose fd nginx
        // isn't polling would never be accepted — the client just times out.
        if !stack::claim_listen_port(endpoint.port) {
            bail!(EADDRINUSE);
        }

        let pool_size = backlog.clamp(1, LISTEN_POOL);
        for _ in 0..pool_size {
            let Some(handle) = self.spawn_listener(endpoint) else {
                break;
            };
            inner.pool.push(handle);
        }
        if inner.pool.is_empty() {
            stack::release_listen_port(endpoint.port);
            bail!(EADDRINUSE);
        }
        inner.backlog = backlog;
        inner.state = SockState::Listening;
        crate::info!(
            "socket: listening on port {} with {} parked sockets",
            endpoint.port,
            inner.pool.len()
        );
        Ok(())
    }

    /// Create one smoltcp socket listening on `endpoint`.
    fn spawn_listener(&self, endpoint: IpListenEndpoint) -> Option<SocketHandle> {
        let handle = stack::add_tcp_socket(
            self.rcvbuf.load(Ordering::Relaxed),
            self.sndbuf.load(Ordering::Relaxed),
        )?;
        let ok = stack::with_tcp(handle, |socket| {
            if self.nodelay.load(Ordering::Relaxed) {
                socket.set_nagle_enabled(false);
            }
            // Without a keepalive/timeout, a half-open connection would hold a
            // pool slot forever.
            socket.set_timeout(Some(smoltcp::time::Duration::from_secs(120)));
            socket.listen(endpoint).is_ok()
        })?;
        if ok {
            Some(handle)
        } else {
            stack::remove_socket(handle);
            None
        }
    }

    /// Look through the pool for a socket that has become connected.
    ///
    /// Returns the accepted socket, or `None` if nothing is pending.
    fn try_accept(&self) -> Option<Arc<Socket>> {
        let mut inner = self.inner.lock();
        if inner.inert {
            return None;
        }
        let endpoint = inner.listen_endpoint?;

        let mut found = None;
        for (i, &handle) in inner.pool.iter().enumerate() {
            let ready = stack::with_tcp(handle, |socket| {
                // `may_send() || may_recv()` covers ESTABLISHED and the
                // half-closed states where data is still readable.
                socket.is_active() && (socket.may_recv() || socket.may_send())
            })
            .unwrap_or(false);
            if ready {
                found = Some((i, handle));
                break;
            }
        }
        let (index, handle) = found?;
        inner.pool.remove(index);

        // Replace it so the backlog stays populated.
        if let Some(new_handle) = self.spawn_listener(endpoint) {
            inner.pool.push(new_handle);
        }

        let (local, peer) = stack::with_tcp(handle, |socket| {
            (
                socket.local_endpoint().map(SockAddr::from_endpoint),
                socket.remote_endpoint().map(SockAddr::from_endpoint),
            )
        })
        .unwrap_or((None, None));

        // Apply the listener's options to the accepted socket, as Linux does.
        if self.nodelay.load(Ordering::Relaxed) {
            stack::with_tcp(handle, |socket| socket.set_nagle_enabled(false));
        }

        let local = local.unwrap_or(SockAddr::V4 {
            addr: Some(stack::local_ip()),
            port: endpoint.port,
        });
        let peer = peer.unwrap_or(SockAddr::V4 {
            addr: None,
            port: 0,
        });
        drop(inner);

        Some(Socket::from_accepted(self.family, handle, local, peer))
    }

    /// `accept`. Blocks unless the socket is non-blocking.
    pub fn accept(&self, nonblock: bool) -> Result<Arc<Socket>> {
        if self.kind != SocketKind::Tcp {
            bail!(EOPNOTSUPP);
        }
        if self.state() != SockState::Listening {
            bail!(EINVAL);
        }
        loop {
            stack::poll();
            if let Some(socket) = self.try_accept() {
                return Ok(socket);
            }
            if nonblock || self.is_nonblock() {
                bail!(EAGAIN);
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// `connect`.
    pub fn connect(&self, addr: SockAddr) -> Result<()> {
        if self.kind == SocketKind::Other {
            bail!(ECONNREFUSED);
        }
        let remote = addr.to_endpoint()?;

        if self.kind == SocketKind::Udp {
            let mut inner = self.inner.lock();
            if inner.handle.is_none() {
                let handle = stack::add_udp_socket(
                    self.rcvbuf.load(Ordering::Relaxed),
                    self.sndbuf.load(Ordering::Relaxed),
                )
                .ok_or(fs::Error::new(fs::errno::ENOBUFS))?;
                let port = stack::ephemeral_port();
                stack::with_udp(handle, |s| {
                    let _ = s.bind(port);
                });
                inner.handle = Some(handle);
                inner.local = Some(SockAddr::V4 {
                    addr: Some(stack::local_ip()),
                    port,
                });
            }
            inner.peer = Some(addr);
            inner.state = SockState::Connected;
            return Ok(());
        }

        // TCP.
        let (handle, already) = {
            let mut inner = self.inner.lock();
            match inner.state {
                SockState::Connected => return Err(fs::Error::new(fs::errno::EISCONN)),
                SockState::Connecting => (inner.handle.unwrap(), true),
                SockState::Listening => bail!(EISCONN),
                _ => {
                    let handle = stack::add_tcp_socket(
                        self.rcvbuf.load(Ordering::Relaxed),
                        self.sndbuf.load(Ordering::Relaxed),
                    )
                    .ok_or(fs::Error::new(fs::errno::ENOBUFS))?;
                    inner.handle = Some(handle);
                    (handle, false)
                }
            }
        };

        if !already {
            let local_port = self
                .inner
                .lock()
                .local
                .as_ref()
                .map(|a| a.port())
                .filter(|&p| p != 0)
                .unwrap_or_else(stack::ephemeral_port);

            let result = stack::with_stack(|s| {
                let socket = s.sockets.get_mut::<tcp::Socket>(handle);
                if self.nodelay.load(Ordering::Relaxed) {
                    socket.set_nagle_enabled(false);
                }
                socket.connect(s.iface.context(), remote, local_port)
            })
            .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;

            if result.is_err() {
                bail!(ECONNREFUSED);
            }
            let mut inner = self.inner.lock();
            inner.state = SockState::Connecting;
            inner.peer = Some(addr.clone());
            inner.local = Some(SockAddr::V4 {
                addr: Some(stack::local_ip()),
                port: local_port,
            });
        }

        // Wait for the handshake.
        if self.is_nonblock() {
            stack::poll();
            if self.check_connected(handle)? {
                return Ok(());
            }
            bail!(EINPROGRESS);
        }

        loop {
            stack::poll();
            if self.check_connected(handle)? {
                return Ok(());
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// Has a connecting socket finished? `Err` if the connection failed.
    fn check_connected(&self, handle: SocketHandle) -> Result<bool> {
        let state = stack::with_tcp(handle, |socket| socket.state())
            .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;
        match state {
            tcp::State::Established => {
                let mut inner = self.inner.lock();
                inner.state = SockState::Connected;
                if let Some(peer) = stack::with_tcp(handle, |s| s.remote_endpoint()).flatten() {
                    inner.peer = Some(SockAddr::from_endpoint(peer));
                }
                Ok(true)
            }
            tcp::State::Closed => {
                self.inner.lock().state = SockState::Closed;
                self.error
                    .store(fs::errno::ECONNREFUSED as i32, Ordering::Relaxed);
                bail!(ECONNREFUSED)
            }
            _ => Ok(false),
        }
    }

    /// Receive data.
    pub fn recv(&self, buf: &mut [u8], nonblock: bool, peek: bool) -> Result<usize> {
        match self.kind {
            SocketKind::Tcp => self.recv_tcp(buf, nonblock, peek),
            SocketKind::Udp => self.recv_udp(buf, nonblock).map(|(n, _)| n),
            SocketKind::Other => Ok(0),
        }
    }

    fn recv_tcp(&self, buf: &mut [u8], nonblock: bool, peek: bool) -> Result<usize> {
        if self.shut_rd.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let handle = self
            .inner
            .lock()
            .handle
            .ok_or(fs::Error::new(fs::errno::ENOTCONN))?;
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = self.timeout_deadline(self.rcvtimeo_ms.load(Ordering::Relaxed));

        loop {
            stack::poll();
            // Read the smoltcp state without holding our own lock, so the closure
            // stays free of lock-ordering hazards.
            let outcome = stack::with_tcp(handle, |socket| {
                if socket.can_recv() {
                    let result = if peek {
                        socket.peek_slice(buf)
                    } else {
                        socket.recv_slice(buf)
                    };
                    match result {
                        Ok(n) => RecvOutcome::Data(n),
                        Err(_) => RecvOutcome::Eof,
                    }
                } else {
                    // No buffered data. Whether that is EOF or merely "not yet"
                    // depends on the TCP state; reporting EOF on an idle
                    // keep-alive connection makes nginx close it after one
                    // request.
                    match socket.state() {
                        // The peer sent FIN and we have drained the buffer.
                        tcp::State::CloseWait
                        | tcp::State::LastAck
                        | tcp::State::Closing
                        | tcp::State::TimeWait => RecvOutcome::Eof,
                        tcp::State::Closed => RecvOutcome::Reset,
                        // Established (idle keep-alive), or still handshaking.
                        _ => RecvOutcome::WouldBlock,
                    }
                }
            })
            .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;

            match outcome {
                RecvOutcome::Data(n) => return Ok(n),
                RecvOutcome::Eof => {
                    // Don't mark the socket Closed: the peer is done sending, but
                    // we may still have a response to write. Only `send` and the
                    // connect path change the recorded state.
                    return Ok(0);
                }
                RecvOutcome::Reset => {
                    let was_connected = {
                        let mut inner = self.inner.lock();
                        let was = inner.state == SockState::Connected;
                        inner.state = SockState::Closed;
                        was
                    };
                    // A connection that reached Closed without a FIN was reset;
                    // one we had already torn down just reports EOF.
                    if was_connected {
                        bail!(ECONNRESET)
                    }
                    return Ok(0);
                }
                RecvOutcome::WouldBlock => {}
            }

            if nonblock || self.is_nonblock() {
                bail!(EAGAIN);
            }
            if let Some(deadline) = deadline {
                if crate::time::monotonic_ms() >= deadline {
                    bail!(EAGAIN);
                }
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// Receive a datagram, returning (bytes, sender).
    pub fn recv_udp(&self, buf: &mut [u8], nonblock: bool) -> Result<(usize, Option<SockAddr>)> {
        let handle = self
            .inner
            .lock()
            .handle
            .ok_or(fs::Error::new(fs::errno::ENOTCONN))?;
        let deadline = self.timeout_deadline(self.rcvtimeo_ms.load(Ordering::Relaxed));
        loop {
            stack::poll();
            let got = stack::with_udp(handle, |socket| {
                if socket.can_recv() {
                    match socket.recv_slice(buf) {
                        Ok((n, meta)) => Some((n, Some(SockAddr::from_endpoint(meta.endpoint)))),
                        Err(_) => None,
                    }
                } else {
                    None
                }
            })
            .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;

            if let Some(result) = got {
                return Ok(result);
            }
            if nonblock || self.is_nonblock() {
                bail!(EAGAIN);
            }
            if let Some(deadline) = deadline {
                if crate::time::monotonic_ms() >= deadline {
                    bail!(EAGAIN);
                }
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// Send data.
    pub fn send(&self, buf: &[u8], nonblock: bool) -> Result<usize> {
        match self.kind {
            SocketKind::Tcp => self.send_tcp(buf, nonblock),
            SocketKind::Udp => {
                let peer = self
                    .inner
                    .lock()
                    .peer
                    .clone()
                    .ok_or(fs::Error::new(EDESTADDRREQ))?;
                self.send_to(buf, &peer, nonblock)
            }
            SocketKind::Other => Ok(buf.len()),
        }
    }

    fn send_tcp(&self, buf: &[u8], nonblock: bool) -> Result<usize> {
        if self.shut_wr.load(Ordering::Relaxed) {
            crate::task::send_signal_to_self(crate::signal::SIGPIPE);
            bail!(EPIPE);
        }
        let handle = self
            .inner
            .lock()
            .handle
            .ok_or(fs::Error::new(fs::errno::ENOTCONN))?;
        if buf.is_empty() {
            return Ok(0);
        }
        let deadline = self.timeout_deadline(self.sndtimeo_ms.load(Ordering::Relaxed));

        loop {
            stack::poll();
            let outcome = stack::with_tcp(handle, |socket| {
                // `may_send` is false in states where the local side has already
                // sent a FIN, and also — importantly — before the handshake
                // completes and in TIME_WAIT. Only treat states from which no
                // further data can ever flow as broken; a socket that is merely
                // not yet ready should block instead, or nginx sees a spurious
                // EPIPE mid-response and aborts the connection.
                match socket.state() {
                    tcp::State::Closed
                    | tcp::State::Closing
                    | tcp::State::LastAck
                    | tcp::State::FinWait1
                    | tcp::State::FinWait2
                    | tcp::State::TimeWait => SendOutcome::Broken,
                    tcp::State::Established | tcp::State::CloseWait => {
                        // CloseWait means the peer sent FIN but we may still send.
                        if socket.can_send() {
                            match socket.send_slice(buf) {
                                Ok(0) => SendOutcome::WouldBlock,
                                Ok(n) => SendOutcome::Sent(n),
                                Err(_) => SendOutcome::Broken,
                            }
                        } else {
                            SendOutcome::WouldBlock
                        }
                    }
                    // SYN-SENT / SYN-RECEIVED / LISTEN: not ready yet.
                    _ => SendOutcome::WouldBlock,
                }
            })
            .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;

            match outcome {
                SendOutcome::Sent(n) => {
                    // Push the data out now rather than waiting for the next
                    // poll, which keeps latency low for small responses.
                    stack::poll();
                    return Ok(n);
                }
                SendOutcome::Broken => {
                    self.inner.lock().state = SockState::Closed;
                    crate::task::send_signal_to_self(crate::signal::SIGPIPE);
                    bail!(EPIPE)
                }
                SendOutcome::WouldBlock => {}
            }

            if nonblock || self.is_nonblock() {
                bail!(EAGAIN);
            }
            if let Some(deadline) = deadline {
                if crate::time::monotonic_ms() >= deadline {
                    bail!(EAGAIN);
                }
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// `sendto`.
    pub fn send_to(&self, buf: &[u8], addr: &SockAddr, nonblock: bool) -> Result<usize> {
        if self.kind == SocketKind::Tcp {
            return self.send_tcp(buf, nonblock);
        }
        if self.kind == SocketKind::Other {
            return Ok(buf.len());
        }
        let endpoint = addr.to_endpoint()?;
        // Bind lazily if the program never called `bind`.
        let handle = {
            let mut inner = self.inner.lock();
            match inner.handle {
                Some(h) => h,
                None => {
                    let handle = stack::add_udp_socket(
                        self.rcvbuf.load(Ordering::Relaxed),
                        self.sndbuf.load(Ordering::Relaxed),
                    )
                    .ok_or(fs::Error::new(fs::errno::ENOBUFS))?;
                    let port = stack::ephemeral_port();
                    stack::with_udp(handle, |s| {
                        let _ = s.bind(port);
                    });
                    inner.handle = Some(handle);
                    inner.local = Some(SockAddr::V4 {
                        addr: Some(stack::local_ip()),
                        port,
                    });
                    handle
                }
            }
        };

        loop {
            let sent = stack::with_udp(handle, |socket| socket.send_slice(buf, endpoint).is_ok())
                .ok_or(fs::Error::new(fs::errno::ENETDOWN))?;
            if sent {
                stack::poll();
                return Ok(buf.len());
            }
            if nonblock || self.is_nonblock() {
                bail!(EAGAIN);
            }
            stack::poll_and_yield();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    /// `shutdown`.
    pub fn shutdown(&self, how: i32) -> Result<()> {
        const SHUT_RD: i32 = 0;
        const SHUT_WR: i32 = 1;
        const SHUT_RDWR: i32 = 2;
        if how == SHUT_RD || how == SHUT_RDWR {
            self.shut_rd.store(true, Ordering::Relaxed);
        }
        if how == SHUT_WR || how == SHUT_RDWR {
            self.shut_wr.store(true, Ordering::Relaxed);
            if let Some(handle) = self.inner.lock().handle {
                if self.kind == SocketKind::Tcp {
                    stack::with_tcp(handle, |socket| socket.close());
                    stack::poll();
                }
            }
        }
        Ok(())
    }

    pub fn local_addr(&self) -> Option<SockAddr> {
        let inner = self.inner.lock();
        if let Some(local) = &inner.local {
            return Some(local.clone());
        }
        // An unbound connected socket: ask smoltcp.
        let handle = inner.handle?;
        drop(inner);
        if self.kind == SocketKind::Tcp {
            stack::with_tcp(handle, |s| s.local_endpoint())
                .flatten()
                .map(SockAddr::from_endpoint)
        } else {
            None
        }
    }

    pub fn peer_addr(&self) -> Option<SockAddr> {
        let inner = self.inner.lock();
        if let Some(peer) = &inner.peer {
            if inner.state == SockState::Connected {
                return Some(peer.clone());
            }
        }
        None
    }

    /// Bytes available to read.
    pub fn available(&self) -> usize {
        let Some(handle) = self.inner.lock().handle else {
            return 0;
        };
        match self.kind {
            SocketKind::Tcp => stack::with_tcp(handle, |s| s.recv_queue()).unwrap_or(0),
            SocketKind::Udp => stack::with_udp(handle, |s| if s.can_recv() { 1 } else { 0 })
                .unwrap_or(0),
            SocketKind::Other => 0,
        }
    }
}

enum RecvOutcome {
    Data(usize),
    Eof,
    Reset,
    WouldBlock,
}

enum SendOutcome {
    Sent(usize),
    Broken,
    WouldBlock,
}

impl Inode for Socket {
    fn kind(&self) -> InodeKind {
        InodeKind::Socket
    }

    fn ino(&self) -> u64 {
        self.ino
    }

    fn mode(&self) -> u32 {
        0o777
    }

    fn read(&self, _offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        self.recv(buf, nonblock, false)
    }

    fn write(&self, _offset: usize, buf: &[u8], nonblock: bool) -> Result<usize> {
        self.send(buf, nonblock)
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.recv(buf, self.is_nonblock(), false)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        self.send(buf, self.is_nonblock())
    }

    fn poll_readable(&self) -> bool {
        // Poll the stack so readiness reflects freshly arrived data. `epoll_wait`
        // and `poll` depend on this being current.
        stack::poll();
        let inner = self.inner.lock();
        if inner.inert {
            // Never ready: an inert listener would otherwise wake nginx's event
            // loop for an `accept` that always returns EAGAIN.
            return false;
        }
        match inner.state {
            SockState::Listening => {
                let pool = inner.pool.clone();
                drop(inner);
                pool.iter().any(|&h| {
                    stack::with_tcp(h, |s| s.is_active() && (s.may_recv() || s.may_send()))
                        .unwrap_or(false)
                })
            }
            _ => {
                let Some(handle) = inner.handle else {
                    return false;
                };
                drop(inner);
                match self.kind {
                    SocketKind::Tcp => stack::with_tcp(handle, |s| {
                        // Readable when data is buffered, or when a read would
                        // return EOF so the reader learns the peer is done. An
                        // idle established connection is *not* readable — saying
                        // it is would spin nginx's event loop.
                        s.can_recv()
                            || matches!(
                                s.state(),
                                tcp::State::CloseWait
                                    | tcp::State::LastAck
                                    | tcp::State::Closing
                                    | tcp::State::TimeWait
                                    | tcp::State::Closed
                            )
                    })
                    .unwrap_or(false),
                    SocketKind::Udp => stack::with_udp(handle, |s| s.can_recv()).unwrap_or(false),
                    SocketKind::Other => false,
                }
            }
        }
    }

    fn poll_writable(&self) -> bool {
        stack::poll();
        let inner = self.inner.lock();
        let Some(handle) = inner.handle else {
            return false;
        };
        let state = inner.state;
        drop(inner);
        match self.kind {
            SocketKind::Tcp => stack::with_tcp(handle, |s| match state {
                // A connecting socket becomes writable when the handshake
                // completes, which is how non-blocking `connect` is detected.
                SockState::Connecting => s.state() == tcp::State::Established,
                _ => s.can_send(),
            })
            .unwrap_or(false),
            SocketKind::Udp => stack::with_udp(handle, |s| s.can_send()).unwrap_or(false),
            SocketKind::Other => true,
        }
    }

    fn poll_hangup(&self) -> bool {
        let inner = self.inner.lock();
        let Some(handle) = inner.handle else {
            return false;
        };
        if self.kind != SocketKind::Tcp {
            return false;
        }
        drop(inner);
        let hung = stack::with_tcp(handle, |s| {
            matches!(
                s.state(),
                tcp::State::Closed | tcp::State::CloseWait | tcp::State::TimeWait
            )
        })
        .unwrap_or(false);
        if hung {
            crate::trace!(
                "poll_hangup: socket {} reports hangup, tcp state {:?}",
                self.ino,
                stack::with_tcp(handle, |s| s.state()),
            );
        }
        hung
    }

    fn poll_error(&self) -> bool {
        self.error.load(Ordering::Relaxed) != 0
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        const FIONREAD: usize = 0x541b;
        const FIONBIO: usize = 0x5421;
        match cmd {
            FIONREAD => {
                crate::mm::uaccess::write(arg, self.available() as u32)?;
                Ok(0)
            }
            FIONBIO => {
                let on: u32 = crate::mm::uaccess::read(arg)?;
                self.nonblock.store(on != 0, Ordering::Relaxed);
                Ok(0)
            }
            _ => bail!(ENOTTY),
        }
    }

    impl_as_any!();
}

impl Drop for Socket {
    fn drop(&mut self) {
        let inner = self.inner.lock();
        if self.kind == SocketKind::Tcp {
            if let Some(handle) = inner.handle {
                // Close cleanly so the peer sees a FIN, then let the stack run
                // once to put the segment on the wire.
                stack::with_tcp(handle, |socket| socket.close());
            }
        }
        let handles: Vec<SocketHandle> = inner
            .handle
            .into_iter()
            .chain(inner.pool.iter().copied())
            .collect();
        // Release the bound port so a restarted listener can reclaim it. Only a
        // socket that owns the binding (bound or listening) should do this — an
        // accepted connection shares the listener's port.
        if inner.state == SockState::Listening && !inner.inert {
            if let Some(endpoint) = inner.listen_endpoint {
                stack::release_listen_port(endpoint.port);
            }
        }
        let owned_port = match inner.state {
            SockState::Bound | SockState::Listening => inner.local.as_ref().map(|a| a.port()),
            SockState::Connecting | SockState::Connected | SockState::Closed => {
                // A client socket owns its ephemeral local port, but an accepted
                // one does not: it has no `listen_endpoint`.
                if inner.listen_endpoint.is_some() {
                    inner.local.as_ref().map(|a| a.port())
                } else {
                    None
                }
            }
            SockState::Unbound => None,
        };
        drop(inner);
        stack::poll();
        for handle in handles {
            stack::remove_socket(handle);
        }
        if let Some(port) = owned_port.filter(|&p| p != 0) {
            stack::release_port(port);
        }
    }
}

