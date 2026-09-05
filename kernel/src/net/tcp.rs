//! TCP sockets on smoltcp.
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp::{self, State};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

use super::socket::{i32_opt, Ancillary, SockAddr, SocketOps};
use super::{NET_WQ, ORPHANS, STACK, TCP_SOCKETS};
use crate::abi::*;
use crate::config::IP_ADDR;
use crate::fs::file::{File, FileOps};
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

const RX_BUF: usize = 128 * 1024;
const TX_BUF: usize = 128 * 1024;
const MAX_BACKLOG: usize = 32;

#[derive(Clone)]
enum Kind {
    Unbound,
    Bound(IpListenEndpoint),
    Listening { local: IpListenEndpoint, handles: Vec<SocketHandle> },
    Connecting(SocketHandle),
    Connected(SocketHandle),
    Closed,
}

struct Inner {
    kind: Kind,
    nodelay: bool,
    keepalive: bool,
    shut_rd: bool,
    shut_wr: bool,
    error: i32,
    backlog: usize,
}

pub struct TcpSocket {
    inner: SpinLock<Inner>,
    seq: AtomicU64,
    /// Last observed (state, rx, tx) snapshot for change detection.
    snapshot: SpinLock<(u8, usize, usize, bool)>,
}

fn new_smol_socket() -> tcp::Socket<'static> {
    let rx = tcp::SocketBuffer::new(alloc::vec![0u8; RX_BUF]);
    let tx = tcp::SocketBuffer::new(alloc::vec![0u8; TX_BUF]);
    let mut s = tcp::Socket::new(rx, tx);
    s.set_nagle_enabled(false);
    s.set_ack_delay(None);
    s
}

fn ep_to_sockaddr(ep: IpEndpoint) -> SockAddr {
    match ep.addr {
        IpAddress::Ipv4(a) => SockAddr::Inet { addr: a.octets(), port: ep.port },
        #[allow(unreachable_patterns)]
        _ => SockAddr::Inet { addr: [0; 4], port: ep.port },
    }
}

fn listen_to_sockaddr(ep: IpListenEndpoint) -> SockAddr {
    match ep.addr {
        Some(IpAddress::Ipv4(a)) => SockAddr::Inet { addr: a.octets(), port: ep.port },
        _ => SockAddr::Inet { addr: [0; 4], port: ep.port },
    }
}

fn is_our_addr(addr: [u8; 4]) -> bool {
    addr == [0, 0, 0, 0] || addr == IP_ADDR || addr == [127, 0, 0, 1]
}

fn state_u8(s: State) -> u8 {
    match s {
        State::Closed => 0,
        State::Listen => 1,
        State::SynSent => 2,
        State::SynReceived => 3,
        State::Established => 4,
        State::FinWait1 => 5,
        State::FinWait2 => 6,
        State::CloseWait => 7,
        State::Closing => 8,
        State::LastAck => 9,
        State::TimeWait => 10,
    }
}

impl TcpSocket {
    pub fn new() -> Arc<TcpSocket> {
        let s = Arc::new(TcpSocket {
            inner: SpinLock::new(Inner {
                kind: Kind::Unbound,
                nodelay: false,
                keepalive: false,
                shut_rd: false,
                shut_wr: false,
                error: 0,
                backlog: 8,
            }),
            seq: AtomicU64::new(1),
            snapshot: SpinLock::new((0, 0, 0, false)),
        });
        let mut list = TCP_SOCKETS.lock();
        list.retain(|w| w.strong_count() > 0);
        list.push(Arc::downgrade(&s));
        s
    }

    fn from_handle(handle: SocketHandle) -> Arc<TcpSocket> {
        let s = TcpSocket::new();
        s.inner.lock().kind = Kind::Connected(handle);
        s
    }

    /// Compute readiness / snapshot without blocking. Must be called with STACK unlocked.
    fn readiness(&self) -> (u32, (u8, usize, usize, bool)) {
        let inner = self.inner.lock();
        let kind = inner.kind.clone();
        let err = inner.error;
        let shut_rd = inner.shut_rd;
        drop(inner);
        let stack = STACK.get().lock();
        match kind {
            Kind::Listening { handles, .. } => {
                let mut n = 0usize;
                for h in &handles {
                    let s = stack.sockets.get::<tcp::Socket>(*h);
                    if Self::acceptable(s) {
                        n += 1;
                    }
                }
                let ev = if n > 0 { POLLIN } else { 0 };
                (ev, (1, n, 0, false))
            }
            Kind::Connected(h) | Kind::Connecting(h) => {
                let s = stack.sockets.get::<tcp::Socket>(h);
                let st = s.state();
                let rx = s.recv_queue();
                let tx = s.send_queue();
                let mut ev = 0;
                if rx > 0 || shut_rd {
                    ev |= POLLIN;
                }
                let peer_closed = matches!(st, State::CloseWait | State::LastAck | State::Closing | State::TimeWait | State::Closed);
                if peer_closed {
                    ev |= POLLIN | POLLRDHUP;
                }
                if matches!(st, State::Closed) {
                    ev |= POLLHUP;
                }
                if s.can_send() && !matches!(st, State::SynSent | State::SynReceived) {
                    ev |= POLLOUT;
                }
                if err != 0 {
                    ev |= POLLERR;
                }
                (ev, (state_u8(st), rx, tx, s.can_send()))
            }
            Kind::Closed => (POLLHUP | POLLIN | POLLOUT, (0, 0, 0, false)),
            Kind::Unbound | Kind::Bound(_) => (POLLOUT | POLLHUP, (0, 0, 0, false)),
        }
    }

    fn acceptable(s: &tcp::Socket) -> bool {
        !matches!(s.state(), State::Listen | State::SynReceived | State::SynSent | State::Closed)
    }

    fn check_changed(&self) -> bool {
        let (_, snap) = self.readiness();
        let mut cur = self.snapshot.lock();
        if *cur != snap {
            *cur = snap;
            self.seq.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    fn do_recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> SysResult {
        if buf.is_empty() {
            return Ok(0);
        }
        let peek = flags & MSG_PEEK != 0;
        block_on(&[&NET_WQ], nonblock || flags & MSG_DONTWAIT != 0, || {
            let (h, shut_rd) = {
                let inner = self.inner.lock();
                match inner.kind {
                    Kind::Connected(h) => (h, inner.shut_rd),
                    Kind::Connecting(_) => return Err(EAGAIN),
                    Kind::Closed => return Ok(0),
                    _ => return Err(ENOTCONN),
                }
            };
            super::poll();
            let mut stack = STACK.get().lock();
            let s = stack.sockets.get_mut::<tcp::Socket>(h);
            if s.recv_queue() > 0 {
                let n = if peek { s.peek_slice(buf).unwrap_or(0) } else { s.recv_slice(buf).unwrap_or(0) };
                return Ok(n);
            }
            if shut_rd || !s.may_recv() {
                // peer closed or reset
                if s.state() == State::Closed && s.recv_queue() == 0 {
                    let err = self.inner.lock().error;
                    if err != 0 {
                        self.inner.lock().error = 0;
                        return Err(err);
                    }
                }
                return Ok(0);
            }
            Err(EAGAIN)
        })
    }

    fn do_send(&self, buf: &[u8], flags: u32, nonblock: bool) -> SysResult {
        if buf.is_empty() {
            return Ok(0);
        }
        let nonblock = nonblock || flags & MSG_DONTWAIT != 0;
        let mut sent = 0usize;
        loop {
            let r = block_on(&[&NET_WQ], nonblock, || {
                let h = {
                    let inner = self.inner.lock();
                    if inner.shut_wr {
                        return Err(EPIPE);
                    }
                    match inner.kind {
                        Kind::Connected(h) => h,
                        Kind::Connecting(_) => return Err(EAGAIN),
                        Kind::Closed => return Err(EPIPE),
                        _ => return Err(ENOTCONN),
                    }
                };
                let mut stack = STACK.get().lock();
                let s = stack.sockets.get_mut::<tcp::Socket>(h);
                if !s.may_send() {
                    return Err(EPIPE);
                }
                if !s.can_send() {
                    return Err(EAGAIN);
                }
                match s.send_slice(&buf[sent..]) {
                    Ok(0) => Err(EAGAIN),
                    Ok(n) => Ok(n),
                    Err(_) => Err(EPIPE),
                }
            });
            match r {
                Ok(n) => {
                    sent += n;
                    super::poll();
                    if sent >= buf.len() || nonblock {
                        return Ok(sent);
                    }
                }
                Err(EAGAIN) if sent > 0 => return Ok(sent),
                Err(EPIPE) => {
                    if sent > 0 {
                        return Ok(sent);
                    }
                    if flags & MSG_NOSIGNAL == 0 {
                        crate::task::signal::send_signal(&crate::task::current(), SIGPIPE, None);
                    }
                    return Err(EPIPE);
                }
                Err(e) => {
                    if sent > 0 {
                        return Ok(sent);
                    }
                    return Err(e);
                }
            }
        }
    }
}

/// Called after each stack poll: bump seq for sockets whose state changed.
/// Returns true if any changed.
pub fn update_event_seqs() -> bool {
    let list: Vec<Arc<TcpSocket>> = TCP_SOCKETS.lock().iter().filter_map(|w| w.upgrade()).collect();
    let mut any = false;
    for s in list {
        if s.check_changed() {
            any = true;
        }
    }
    any
}

impl SocketOps for TcpSocket {
    fn bind(&self, addr: SockAddr) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = addr else { return Err(EINVAL) };
        if !is_our_addr(addr) {
            return Err(EADDRNOTAVAIL);
        }
        let mut inner = self.inner.lock();
        if !matches!(inner.kind, Kind::Unbound) {
            return Err(EINVAL);
        }
        // check for conflicts with other listening sockets
        {
            let stack = STACK.get().lock();
            for (_, s) in stack.sockets.iter() {
                if let smoltcp::socket::Socket::Tcp(t) = s {
                    if t.is_listening() && t.listen_endpoint().port == port && port != 0 {
                        return Err(EADDRINUSE);
                    }
                }
            }
        }
        let ip = if addr == [0, 0, 0, 0] { None } else { Some(IpAddress::Ipv4(Ipv4Address::from_octets(addr))) };
        inner.kind = Kind::Bound(IpListenEndpoint { addr: ip, port });
        Ok(())
    }

    fn listen(&self, backlog: i32) -> Result<(), i32> {
        let mut inner = self.inner.lock();
        let local = match &inner.kind {
            Kind::Bound(ep) => *ep,
            Kind::Listening { .. } => return Ok(()),
            Kind::Unbound => IpListenEndpoint { addr: None, port: alloc_ephemeral_port() },
            _ => return Err(EINVAL),
        };
        if local.port == 0 {
            return Err(EADDRINUSE);
        }
        let n = (backlog.max(1) as usize).min(MAX_BACKLOG);
        inner.backlog = n;
        let mut handles = Vec::new();
        let mut stack = STACK.get().lock();
        for _ in 0..n {
            let mut s = new_smol_socket();
            s.listen(local).map_err(|_| EADDRINUSE)?;
            handles.push(stack.sockets.add(s));
        }
        inner.kind = Kind::Listening { local, handles };
        Ok(())
    }

    fn accept(&self, nonblock: bool) -> Result<(Arc<dyn FileOps>, SockAddr), i32> {
        block_on(&[&NET_WQ], nonblock, || {
            super::poll();
            let mut inner = self.inner.lock();
            let Kind::Listening { local, handles } = &mut inner.kind else { return Err(EINVAL) };
            let local = *local;
            let mut stack = STACK.get().lock();
            let mut found = None;
            for (i, h) in handles.iter().enumerate() {
                let s = stack.sockets.get::<tcp::Socket>(*h);
                if Self::acceptable(s) {
                    found = Some((i, *h, s.remote_endpoint()));
                    break;
                }
            }
            let Some((i, h, remote)) = found else { return Err(EAGAIN) };
            handles.remove(i);
            // replenish the backlog
            let mut s = new_smol_socket();
            if s.listen(local).is_ok() {
                handles.push(stack.sockets.add(s));
            }
            let nodelay = inner.nodelay;
            if nodelay {
                stack.sockets.get_mut::<tcp::Socket>(h).set_nagle_enabled(false);
            }
            drop(stack);
            drop(inner);
            let peer = remote.map(ep_to_sockaddr).unwrap_or(SockAddr::Inet { addr: [0; 4], port: 0 });
            let sock = TcpSocket::from_handle(h);
            Ok((sock as Arc<dyn FileOps>, peer))
        })
    }

    fn connect(&self, addr: SockAddr, nonblock: bool) -> Result<(), i32> {
        let SockAddr::Inet { addr, port } = addr else { return Err(EINVAL) };
        let handle = {
            let mut inner = self.inner.lock();
            match inner.kind.clone() {
                Kind::Unbound | Kind::Bound(_) => {
                    let local_port = match inner.kind {
                        Kind::Bound(ep) if ep.port != 0 => ep.port,
                        _ => alloc_ephemeral_port(),
                    };
                    let mut stack = STACK.get().lock();
                    let Stack { iface, sockets } = &mut *stack;
                    let mut s = new_smol_socket();
                    let remote = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from_octets(addr)), port);
                    s.connect(iface.context(), remote, local_port).map_err(|_| ECONNREFUSED)?;
                    let h = sockets.add(s);
                    inner.kind = Kind::Connecting(h);
                    h
                }
                Kind::Connecting(h) => {
                    if nonblock {
                        // report progress
                        let stack = STACK.get().lock();
                        let s = stack.sockets.get::<tcp::Socket>(h);
                        return match s.state() {
                            State::Established => {
                                drop(stack);
                                inner.kind = Kind::Connected(h);
                                Ok(())
                            }
                            State::Closed => Err(ECONNREFUSED),
                            _ => Err(EALREADY),
                        };
                    }
                    h
                }
                Kind::Connected(_) => return Err(EISCONN),
                _ => return Err(EINVAL),
            }
        };
        super::poll();
        if nonblock {
            return Err(EINPROGRESS);
        }
        block_on(&[&NET_WQ], false, || {
            let stack = STACK.get().lock();
            let s = stack.sockets.get::<tcp::Socket>(handle);
            match s.state() {
                State::SynSent | State::SynReceived => Err(EAGAIN),
                State::Closed => Err(ECONNREFUSED),
                _ => {
                    drop(stack);
                    self.inner.lock().kind = Kind::Connected(handle);
                    Ok(())
                }
            }
        })
    }

    fn send(&self, buf: &[u8], flags: u32, nonblock: bool, _to: Option<SockAddr>, _anc: Ancillary) -> SysResult {
        self.do_send(buf, flags, nonblock)
    }

    fn recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> Result<(usize, Option<SockAddr>, Ancillary), i32> {
        let n = self.do_recv(buf, flags, nonblock)?;
        Ok((n, None, Ancillary::default()))
    }

    fn shutdown(&self, how: i32) -> Result<(), i32> {
        let mut inner = self.inner.lock();
        let h = match inner.kind {
            Kind::Connected(h) => h,
            Kind::Listening { .. } => return Ok(()),
            _ => return Err(ENOTCONN),
        };
        if how == SHUT_RD || how == SHUT_RDWR {
            inner.shut_rd = true;
        }
        if how == SHUT_WR || how == SHUT_RDWR {
            inner.shut_wr = true;
            let mut stack = STACK.get().lock();
            stack.sockets.get_mut::<tcp::Socket>(h).close();
        }
        drop(inner);
        super::poll();
        Ok(())
    }

    fn local_addr(&self) -> Result<SockAddr, i32> {
        let inner = self.inner.lock();
        match &inner.kind {
            Kind::Unbound => Ok(SockAddr::Inet { addr: [0; 4], port: 0 }),
            Kind::Bound(ep) => Ok(listen_to_sockaddr(*ep)),
            Kind::Listening { local, .. } => Ok(listen_to_sockaddr(*local)),
            Kind::Connected(h) | Kind::Connecting(h) => {
                let stack = STACK.get().lock();
                let s = stack.sockets.get::<tcp::Socket>(*h);
                Ok(s.local_endpoint().map(ep_to_sockaddr).unwrap_or(SockAddr::Inet { addr: IP_ADDR, port: 0 }))
            }
            Kind::Closed => Ok(SockAddr::Inet { addr: [0; 4], port: 0 }),
        }
    }

    fn peer_addr(&self) -> Result<SockAddr, i32> {
        let inner = self.inner.lock();
        match &inner.kind {
            Kind::Connected(h) => {
                let stack = STACK.get().lock();
                let s = stack.sockets.get::<tcp::Socket>(*h);
                s.remote_endpoint().map(ep_to_sockaddr).ok_or(ENOTCONN)
            }
            _ => Err(ENOTCONN),
        }
    }

    fn setsockopt(&self, level: i32, opt: i32, val: &[u8]) -> Result<(), i32> {
        let v = if val.len() >= 4 { i32::from_le_bytes(val[..4].try_into().unwrap()) } else { 0 };
        let mut inner = self.inner.lock();
        match (level, opt) {
            (SOL_TCP, TCP_NODELAY) => {
                inner.nodelay = v != 0;
                if let Kind::Connected(h) = inner.kind {
                    let mut stack = STACK.get().lock();
                    stack.sockets.get_mut::<tcp::Socket>(h).set_nagle_enabled(v == 0);
                }
                Ok(())
            }
            (SOL_SOCKET, SO_KEEPALIVE) => {
                inner.keepalive = v != 0;
                if let Kind::Connected(h) = inner.kind {
                    let mut stack = STACK.get().lock();
                    let ka = if v != 0 { Some(smoltcp::time::Duration::from_secs(60)) } else { None };
                    stack.sockets.get_mut::<tcp::Socket>(h).set_keep_alive(ka);
                }
                Ok(())
            }
            (SOL_SOCKET, _) | (SOL_TCP, _) | (SOL_IP, _) => Ok(()),
            _ => Err(ENOPROTOOPT),
        }
    }

    fn getsockopt(&self, level: i32, opt: i32) -> Result<Vec<u8>, i32> {
        let mut inner = self.inner.lock();
        match (level, opt) {
            (SOL_SOCKET, SO_TYPE) => Ok(i32_opt(SOCK_STREAM as i32)),
            (SOL_SOCKET, SO_ERROR) => {
                let e = inner.error;
                inner.error = 0;
                // connection in progress finished?
                if let Kind::Connecting(h) = inner.kind {
                    let stack = STACK.get().lock();
                    let s = stack.sockets.get::<tcp::Socket>(h);
                    match s.state() {
                        State::Established => {
                            drop(stack);
                            inner.kind = Kind::Connected(h);
                            return Ok(i32_opt(0));
                        }
                        State::Closed => return Ok(i32_opt(ECONNREFUSED)),
                        _ => return Ok(i32_opt(EINPROGRESS)),
                    }
                }
                Ok(i32_opt(e))
            }
            (SOL_SOCKET, SO_DOMAIN) => Ok(i32_opt(AF_INET as i32)),
            (SOL_SOCKET, SO_PROTOCOL) => Ok(i32_opt(6)),
            (SOL_SOCKET, SO_ACCEPTCONN) => Ok(i32_opt(matches!(inner.kind, Kind::Listening { .. }) as i32)),
            (SOL_SOCKET, SO_RCVBUF) => Ok(i32_opt(RX_BUF as i32)),
            (SOL_SOCKET, SO_SNDBUF) => Ok(i32_opt(TX_BUF as i32)),
            (SOL_SOCKET, SO_REUSEADDR) | (SOL_SOCKET, SO_REUSEPORT) => Ok(i32_opt(1)),
            (SOL_SOCKET, SO_KEEPALIVE) => Ok(i32_opt(inner.keepalive as i32)),
            (SOL_TCP, TCP_NODELAY) => Ok(i32_opt(inner.nodelay as i32)),
            (SOL_TCP, TCP_MAXSEG) => Ok(i32_opt(1460)),
            (SOL_SOCKET, SO_LINGER) => Ok(alloc::vec![0u8; 8]),
            _ => Err(ENOPROTOOPT),
        }
    }

    fn sock_type(&self) -> u32 {
        SOCK_STREAM
    }

    fn domain(&self) -> u16 {
        AF_INET
    }
}

impl FileOps for TcpSocket {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        self.do_recv(buf, 0, file.nonblock())
    }

    fn write_at(&self, _off: u64, buf: &[u8], file: &File) -> SysResult {
        self.do_send(buf, 0, file.nonblock())
    }

    fn poll(&self) -> u32 {
        self.readiness().0
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&NET_WQ)
    }

    fn event_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> SysResult {
        match cmd {
            FIONREAD => {
                let inner = self.inner.lock();
                let n = match inner.kind {
                    Kind::Connected(h) => {
                        let stack = STACK.get().lock();
                        stack.sockets.get::<tcp::Socket>(h).recv_queue() as i32
                    }
                    _ => 0,
                };
                drop(inner);
                crate::mm::uaccess::write_val(arg, n)?;
                Ok(0)
            }
            _ => Err(ENOTTY),
        }
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(Stat { st_mode: S_IFSOCK | 0o777, st_nlink: 1, st_blksize: 4096, ..Stat::default() })
    }

    fn as_socket(&self) -> Option<&dyn SocketOps> {
        Some(self)
    }

    fn release(&self) {
        let mut inner = self.inner.lock();
        let kind = core::mem::replace(&mut inner.kind, Kind::Closed);
        drop(inner);
        let mut stack = STACK.get().lock();
        match kind {
            Kind::Listening { handles, .. } => {
                for h in handles {
                    let s = stack.sockets.get_mut::<tcp::Socket>(h);
                    if Self::acceptable(s) {
                        // a pending connection nobody accepted: reset it
                        s.abort();
                        ORPHANS.lock().push(h);
                    } else {
                        stack.sockets.remove(h);
                    }
                }
            }
            Kind::Connected(h) | Kind::Connecting(h) => {
                let s = stack.sockets.get_mut::<tcp::Socket>(h);
                if s.recv_queue() > 0 {
                    // unread data: RST like Linux
                    s.abort();
                } else {
                    s.close();
                }
                ORPHANS.lock().push(h);
            }
            _ => {}
        }
        drop(stack);
        super::poll();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

use super::Stack;

static NEXT_PORT: AtomicU64 = AtomicU64::new(49152);

pub fn alloc_ephemeral_port() -> u16 {
    let p = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    if p >= 65535 {
        NEXT_PORT.store(49152, Ordering::Relaxed);
    }
    (49152 + (p - 49152) % 16000) as u16
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        let _ = Weak::<TcpSocket>::new();
    }
}
