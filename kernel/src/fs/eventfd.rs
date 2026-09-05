//! eventfd.
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use super::file::{File, FileOps};
use crate::abi::*;
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

pub struct EventFd {
    count: SpinLock<u64>,
    semaphore: bool,
    wq: WaitQueue,
    seq: AtomicU64,
}

impl EventFd {
    pub fn new(init: u64, semaphore: bool) -> Self {
        EventFd { count: SpinLock::new(init), semaphore, wq: WaitQueue::new(), seq: AtomicU64::new(0) }
    }
}

impl FileOps for EventFd {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        if buf.len() < 8 {
            return Err(EINVAL);
        }
        block_on(&[&self.wq], file.nonblock(), || {
            let mut c = self.count.lock();
            if *c == 0 {
                return Err(EAGAIN);
            }
            let v = if self.semaphore { 1 } else { *c };
            *c -= v;
            drop(c);
            self.seq.fetch_add(1, Ordering::Relaxed);
            self.wq.wake_all();
            buf[..8].copy_from_slice(&v.to_le_bytes());
            Ok(8)
        })
    }

    fn write_at(&self, _off: u64, buf: &[u8], file: &File) -> SysResult {
        if buf.len() < 8 {
            return Err(EINVAL);
        }
        let v = u64::from_le_bytes(buf[..8].try_into().unwrap());
        if v == u64::MAX {
            return Err(EINVAL);
        }
        block_on(&[&self.wq], file.nonblock(), || {
            let mut c = self.count.lock();
            if u64::MAX - 1 - *c < v {
                return Err(EAGAIN);
            }
            *c += v;
            drop(c);
            self.seq.fetch_add(1, Ordering::Relaxed);
            self.wq.wake_all();
            Ok(8)
        })
    }

    fn poll(&self) -> u32 {
        let c = *self.count.lock();
        let mut ev = 0;
        if c > 0 {
            ev |= POLLIN;
        }
        if c < u64::MAX - 1 {
            ev |= POLLOUT;
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&self.wq)
    }

    fn event_seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(Stat { st_mode: S_IFREG | 0o600, st_nlink: 1, ..Stat::default() })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
