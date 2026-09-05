//! AF_UNIX stream/datagram sockets (socketpair only) with SCM_RIGHTS support.
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::socket::{i32_opt, Ancillary, SockAddr, SocketOps};
use crate::abi::*;
use crate::fs::file::{File, FileOps};
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

const UNIX_BUF_MAX: usize = 256 * 1024;

struct Message {
    data: Vec<u8>,
    pos: usize,
    fds: Vec<Arc<File>>,
}

pub struct Queue {
    msgs: VecDeque<Message>,
    bytes: usize,
}

/// One end of a socketpair. `rx` is our receive queue, `tx` is the peer's.
pub struct UnixSocket {
    rx: Arc<SpinLock<Queue>>,
    tx: Arc<SpinLock<Queue>>,
    rx_wq: Arc<WaitQueue>,
    tx_wq: Arc<WaitQueue>,
    /// Set when the peer endpoint is closed (or shut down for writing).
    peer_closed: Arc<AtomicBool>,
    /// Set when we are closed (peer sees EPIPE).
    self_closed: Arc<AtomicBool>,
    rx_seq: Arc<AtomicU64>,
    tx_seq: Arc<AtomicU64>,
    stream: bool,
    shut_rd: AtomicBool,
    shut_wr: AtomicBool,
}

pub fn socketpair(stream: bool) -> (Arc<UnixSocket>, Arc<UnixSocket>) {
    let qa = Arc::new(SpinLock::new(Queue { msgs: VecDeque::new(), bytes: 0 }));
    let qb = Arc::new(SpinLock::new(Queue { msgs: VecDeque::new(), bytes: 0 }));
    let wqa = Arc::new(WaitQueue::new());
    let wqb = Arc::new(WaitQueue::new());
    let ca = Arc::new(AtomicBool::new(false));
    let cb = Arc::new(AtomicBool::new(false));
    let sa = Arc::new(AtomicU64::new(0));
    let sb = Arc::new(AtomicU64::new(0));
    let a = Arc::new(UnixSocket {
        rx: qa.clone(),
        tx: qb.clone(),
        rx_wq: wqa.clone(),
        tx_wq: wqb.clone(),
        peer_closed: cb.clone(),
        self_closed: ca.clone(),
        rx_seq: sa.clone(),
        tx_seq: sb.clone(),
        stream,
        shut_rd: AtomicBool::new(false),
        shut_wr: AtomicBool::new(false),
    });
    let b = Arc::new(UnixSocket {
        rx: qb,
        tx: qa,
        rx_wq: wqb,
        tx_wq: wqa,
        peer_closed: ca,
        self_closed: cb,
        rx_seq: sb,
        tx_seq: sa,
        stream,
        shut_rd: AtomicBool::new(false),
        shut_wr: AtomicBool::new(false),
    });
    (a, b)
}

impl UnixSocket {
    fn do_recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> Result<(usize, Option<SockAddr>, Ancillary), i32> {
        let peek = flags & MSG_PEEK != 0;
        block_on(&[&self.rx_wq], nonblock || flags & MSG_DONTWAIT != 0, || {
            let mut q = self.rx.lock();
            if q.msgs.is_empty() {
                if self.peer_closed.load(Ordering::Relaxed) || self.shut_rd.load(Ordering::Relaxed) {
                    return Ok((0, None, Ancillary::default()));
                }
                return Err(EAGAIN);
            }
            let mut anc = Ancillary::default();
            let mut n = 0;
            if self.stream {
                while n < buf.len() {
                    let Some(m) = q.msgs.front_mut() else { break };
                    // fds are delivered with the first byte of their message
                    if !m.fds.is_empty() && !anc.fds.is_empty() {
                        break;
                    }
                    let avail = m.data.len() - m.pos;
                    let take = avail.min(buf.len() - n);
                    buf[n..n + take].copy_from_slice(&m.data[m.pos..m.pos + take]);
                    n += take;
                    if peek {
                        anc.fds.extend(m.fds.iter().cloned());
                        break;
                    }
                    if !m.fds.is_empty() {
                        anc.fds.append(&mut m.fds);
                    }
                    m.pos += take;
                    q.bytes -= take;
                    if m.pos >= m.data.len() {
                        q.msgs.pop_front();
                    }
                    if !anc.fds.is_empty() {
                        break;
                    }
                }
            } else {
                let m = q.msgs.front_mut().unwrap();
                let take = m.data.len().min(buf.len());
                buf[..take].copy_from_slice(&m.data[..take]);
                n = take;
                if !peek {
                    anc.fds.append(&mut m.fds);
                    q.bytes -= m.data.len();
                    q.msgs.pop_front();
                } else {
                    anc.fds.extend(m.fds.iter().cloned());
                }
            }
            drop(q);
            self.tx_wq.wake_all();
            self.tx_seq.fetch_add(1, Ordering::Relaxed);
            Ok((n, None, anc))
        })
    }
}

impl SocketOps for UnixSocket {
    fn send(&self, buf: &[u8], flags: u32, nonblock: bool, _to: Option<SockAddr>, anc: Ancillary) -> SysResult {
        if self.shut_wr.load(Ordering::Relaxed) {
            return Err(EPIPE);
        }
        let r = block_on(&[&self.tx_wq], nonblock || flags & MSG_DONTWAIT != 0, || {
            if self.peer_closed.load(Ordering::Relaxed) {
                return Err(EPIPE);
            }
            let mut q = self.tx.lock();
            if q.bytes + buf.len() > UNIX_BUF_MAX && q.bytes > 0 {
                return Err(EAGAIN);
            }
            q.bytes += buf.len();
            q.msgs.push_back(Message { data: buf.to_vec(), pos: 0, fds: Vec::new() });
            Ok(buf.len())
        });
        match r {
            Ok(n) => {
                if !anc.fds.is_empty() {
                    if let Some(m) = self.tx.lock().msgs.back_mut() {
                        m.fds = anc.fds;
                    }
                }
                self.rx_wq.wake_all();
                self.rx_seq.fetch_add(1, Ordering::Relaxed);
                Ok(n)
            }
            Err(EPIPE) => {
                if flags & MSG_NOSIGNAL == 0 {
                    crate::task::signal::send_signal(&crate::task::current(), SIGPIPE, None);
                }
                Err(EPIPE)
            }
            Err(e) => Err(e),
        }
    }

    fn recv(&self, buf: &mut [u8], flags: u32, nonblock: bool) -> Result<(usize, Option<SockAddr>, Ancillary), i32> {
        self.do_recv(buf, flags, nonblock)
    }

    fn shutdown(&self, how: i32) -> Result<(), i32> {
        if how == SHUT_RD || how == SHUT_RDWR {
            self.shut_rd.store(true, Ordering::Relaxed);
            self.rx_wq.wake_all();
        }
        if how == SHUT_WR || how == SHUT_RDWR {
            self.shut_wr.store(true, Ordering::Relaxed);
            self.self_closed.store(true, Ordering::Relaxed);
            self.tx_wq.wake_all();
            self.rx_seq.fetch_add(1, Ordering::Relaxed);
            self.tx_seq.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn local_addr(&self) -> Result<SockAddr, i32> {
        Ok(SockAddr::Unix(Vec::new()))
    }

    fn peer_addr(&self) -> Result<SockAddr, i32> {
        Ok(SockAddr::Unix(Vec::new()))
    }

    fn getsockopt(&self, level: i32, opt: i32) -> Result<Vec<u8>, i32> {
        match (level, opt) {
            (SOL_SOCKET, SO_TYPE) => Ok(i32_opt(self.sock_type() as i32)),
            (SOL_SOCKET, SO_ERROR) => Ok(i32_opt(0)),
            (SOL_SOCKET, SO_DOMAIN) => Ok(i32_opt(AF_UNIX as i32)),
            (SOL_SOCKET, SO_SNDBUF) | (SOL_SOCKET, SO_RCVBUF) => Ok(i32_opt(UNIX_BUF_MAX as i32)),
            (SOL_SOCKET, SO_ACCEPTCONN) => Ok(i32_opt(0)),
            _ => Err(ENOPROTOOPT),
        }
    }

    fn sock_type(&self) -> u32 {
        if self.stream {
            SOCK_STREAM
        } else {
            SOCK_DGRAM
        }
    }

    fn domain(&self) -> u16 {
        AF_UNIX
    }
}

impl FileOps for UnixSocket {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        self.do_recv(buf, 0, file.nonblock()).map(|(n, _, _)| n)
    }

    fn write_at(&self, _off: u64, buf: &[u8], file: &File) -> SysResult {
        self.send(buf, 0, file.nonblock(), None, Ancillary::default())
    }

    fn poll(&self) -> u32 {
        let mut ev = 0;
        let q = self.rx.lock();
        if !q.msgs.is_empty() {
            ev |= POLLIN;
        }
        drop(q);
        if self.peer_closed.load(Ordering::Relaxed) {
            ev |= POLLIN | POLLRDHUP | POLLHUP;
        }
        if self.shut_rd.load(Ordering::Relaxed) {
            ev |= POLLIN | POLLRDHUP;
        }
        if self.tx.lock().bytes < UNIX_BUF_MAX {
            ev |= POLLOUT;
        }
        if self.peer_closed.load(Ordering::Relaxed) && self.self_closed.load(Ordering::Relaxed) {
            ev |= POLLHUP;
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&self.rx_wq)
    }

    fn event_seq(&self) -> u64 {
        self.rx_seq.load(Ordering::Relaxed)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> SysResult {
        match cmd {
            FIONREAD => {
                let n = self.rx.lock().bytes as i32;
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
        self.self_closed.store(true, Ordering::Relaxed);
        // Drop any queued fds destined to us to break reference cycles.
        self.rx.lock().msgs.clear();
        self.tx_wq.wake_all();
        self.rx_wq.wake_all();
        self.tx_seq.fetch_add(1, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
