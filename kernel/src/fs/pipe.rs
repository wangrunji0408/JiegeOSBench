//! Anonymous pipes.
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::file::{File, FileOps};
use crate::abi::*;
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

pub const PIPE_BUF_SIZE: usize = 64 * 1024;

pub struct PipeInner {
    pub buf: VecDeque<u8>,
    pub readers: AtomicUsize,
    pub writers: AtomicUsize,
    pub wq: WaitQueue,
}

pub struct Pipe {
    pub inner: SpinLock<PipeInner>,
    pub wq: WaitQueue,
}

impl Pipe {
    pub fn new() -> Arc<Pipe> {
        Arc::new(Pipe {
            inner: SpinLock::new(PipeInner {
                buf: VecDeque::with_capacity(PIPE_BUF_SIZE),
                readers: AtomicUsize::new(0),
                writers: AtomicUsize::new(0),
                wq: WaitQueue::new(),
            }),
            wq: WaitQueue::new(),
        })
    }
}

pub struct PipeReader {
    pub pipe: Arc<Pipe>,
}
pub struct PipeWriter {
    pub pipe: Arc<Pipe>,
}

pub fn create_pipe() -> (Arc<PipeReader>, Arc<PipeWriter>) {
    let p = Pipe::new();
    p.inner.lock().readers.fetch_add(1, Ordering::Relaxed);
    p.inner.lock().writers.fetch_add(1, Ordering::Relaxed);
    (Arc::new(PipeReader { pipe: p.clone() }), Arc::new(PipeWriter { pipe: p }))
}

fn pipe_stat() -> Stat {
    Stat { st_mode: S_IFIFO | 0o600, st_nlink: 1, st_blksize: 4096, ..Stat::default() }
}

impl FileOps for PipeReader {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        if buf.is_empty() {
            return Ok(0);
        }
        block_on(&[&self.pipe.wq], file.nonblock(), || {
            let mut inner = self.pipe.inner.lock();
            if inner.buf.is_empty() {
                if inner.writers.load(Ordering::Relaxed) == 0 {
                    return Ok(0);
                }
                return Err(EAGAIN);
            }
            let n = buf.len().min(inner.buf.len());
            for b in buf[..n].iter_mut() {
                *b = inner.buf.pop_front().unwrap();
            }
            drop(inner);
            self.pipe.wq.wake_all();
            Ok(n)
        })
    }

    fn poll(&self) -> u32 {
        let inner = self.pipe.inner.lock();
        let mut ev = 0;
        if !inner.buf.is_empty() {
            ev |= POLLIN;
        }
        if inner.writers.load(Ordering::Relaxed) == 0 {
            ev |= POLLHUP;
            if inner.buf.is_empty() {
                ev |= POLLIN;
            }
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&self.pipe.wq)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> SysResult {
        match cmd {
            FIONREAD => {
                let n = self.pipe.inner.lock().buf.len() as i32;
                crate::mm::uaccess::write_val(arg, n)?;
                Ok(0)
            }
            _ => Err(ENOTTY),
        }
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(pipe_stat())
    }

    fn release(&self) {
        self.pipe.inner.lock().readers.fetch_sub(1, Ordering::Relaxed);
        self.pipe.wq.wake_all();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl FileOps for PipeWriter {
    fn write_at(&self, _off: u64, buf: &[u8], file: &File) -> SysResult {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut written = 0;
        let nonblock = file.nonblock();
        loop {
            let r = block_on(&[&self.pipe.wq], nonblock, || {
                let mut inner = self.pipe.inner.lock();
                if inner.readers.load(Ordering::Relaxed) == 0 {
                    return Err(EPIPE);
                }
                let space = PIPE_BUF_SIZE - inner.buf.len();
                if space == 0 {
                    return Err(EAGAIN);
                }
                let n = space.min(buf.len() - written);
                inner.buf.extend(&buf[written..written + n]);
                drop(inner);
                self.pipe.wq.wake_all();
                Ok(n)
            });
            match r {
                Ok(n) => {
                    written += n;
                    if written >= buf.len() {
                        return Ok(written);
                    }
                }
                Err(EPIPE) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    crate::task::signal::send_signal(&crate::task::current(), SIGPIPE, None);
                    return Err(EPIPE);
                }
                Err(e) => {
                    if written > 0 {
                        return Ok(written);
                    }
                    return Err(e);
                }
            }
        }
    }

    fn poll(&self) -> u32 {
        let inner = self.pipe.inner.lock();
        let mut ev = 0;
        if inner.buf.len() < PIPE_BUF_SIZE {
            ev |= POLLOUT;
        }
        if inner.readers.load(Ordering::Relaxed) == 0 {
            ev |= POLLERR;
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&self.pipe.wq)
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(pipe_stat())
    }

    fn release(&self) {
        self.pipe.inner.lock().writers.fetch_sub(1, Ordering::Relaxed);
        self.pipe.wq.wake_all();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
