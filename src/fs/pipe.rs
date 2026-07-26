//! Pipes and FIFOs.
//!
//! nginx uses a pipe pair for its master/worker channel, so blocking reads,
//! `EAGAIN` on empty non-blocking reads, and `EPIPE`/`SIGPIPE` on a closed
//! reader all have to behave.

use super::inode::{next_ino, Inode, InodeKind};
use super::Result;
use crate::{bail, impl_as_any};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use spin::Mutex;

/// Default pipe capacity (Linux uses 64 KiB).
const PIPE_CAPACITY: usize = 64 * 1024;

struct PipeBuffer {
    data: VecDeque<u8>,
    /// Number of open read ends.
    readers: usize,
    /// Number of open write ends.
    writers: usize,
}

/// The shared state of a pipe.
pub struct Pipe {
    buffer: Mutex<PipeBuffer>,
    capacity: AtomicUsize,
}

impl Pipe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buffer: Mutex::new(PipeBuffer {
                data: VecDeque::new(),
                readers: 1,
                writers: 1,
            }),
            capacity: AtomicUsize::new(PIPE_CAPACITY),
        })
    }
}

/// One end of a pipe.
pub struct PipeEnd {
    ino: u64,
    pipe: Arc<Pipe>,
    is_writer: bool,
}

/// Create a pipe, returning (read end, write end).
pub fn new_pipe() -> (Arc<PipeEnd>, Arc<PipeEnd>) {
    let pipe = Pipe::new();
    let read = Arc::new(PipeEnd {
        ino: next_ino(),
        pipe: pipe.clone(),
        is_writer: false,
    });
    let write = Arc::new(PipeEnd {
        ino: next_ino(),
        pipe,
        is_writer: true,
    });
    (read, write)
}

impl Inode for PipeEnd {
    fn kind(&self) -> InodeKind {
        InodeKind::Fifo
    }

    fn ino(&self) -> u64 {
        self.ino
    }

    fn mode(&self) -> u32 {
        0o600
    }

    fn size(&self) -> usize {
        self.pipe.buffer.lock().data.len()
    }

    fn read(&self, _offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        if self.is_writer {
            bail!(EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut b = self.pipe.buffer.lock();
                if !b.data.is_empty() {
                    let n = buf.len().min(b.data.len());
                    for i in 0..n {
                        buf[i] = b.data.pop_front().unwrap();
                    }
                    return Ok(n);
                }
                if b.writers == 0 {
                    // All writers gone and the buffer is drained: EOF.
                    return Ok(0);
                }
            }
            if nonblock {
                bail!(EAGAIN);
            }
            crate::task::yield_now();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    fn write(&self, _offset: usize, buf: &[u8], nonblock: bool) -> Result<usize> {
        if !self.is_writer {
            bail!(EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let capacity = self.pipe.capacity.load(Ordering::Relaxed);
        let mut written = 0;
        loop {
            {
                let mut b = self.pipe.buffer.lock();
                if b.readers == 0 {
                    // Writing to a pipe with no readers.
                    if written > 0 {
                        return Ok(written);
                    }
                    crate::task::send_signal_to_self(crate::signal::SIGPIPE);
                    bail!(EPIPE);
                }
                let space = capacity.saturating_sub(b.data.len());
                if space > 0 {
                    let n = space.min(buf.len() - written);
                    b.data.extend(&buf[written..written + n]);
                    written += n;
                    if written == buf.len() {
                        return Ok(written);
                    }
                }
            }
            // A short write is fine once we've made progress, and matches
            // Linux for non-atomic (> PIPE_BUF) writes.
            if nonblock {
                if written > 0 {
                    return Ok(written);
                }
                bail!(EAGAIN);
            }
            crate::task::yield_now();
            if crate::task::has_pending_signal() {
                if written > 0 {
                    return Ok(written);
                }
                bail!(EINTR);
            }
        }
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.read(0, buf, true)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        self.write(0, buf, true)
    }

    fn poll_readable(&self) -> bool {
        if self.is_writer {
            return false;
        }
        let b = self.pipe.buffer.lock();
        !b.data.is_empty() || b.writers == 0
    }

    fn poll_writable(&self) -> bool {
        if !self.is_writer {
            return false;
        }
        let b = self.pipe.buffer.lock();
        b.readers == 0 || b.data.len() < self.pipe.capacity.load(Ordering::Relaxed)
    }

    fn poll_hangup(&self) -> bool {
        let b = self.pipe.buffer.lock();
        if self.is_writer {
            b.readers == 0
        } else {
            b.writers == 0 && b.data.is_empty()
        }
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        const FIONREAD: usize = 0x541b;
        if cmd == FIONREAD {
            let n = self.pipe.buffer.lock().data.len() as u32;
            crate::mm::uaccess::write(arg, n)?;
            return Ok(0);
        }
        bail!(ENOTTY)
    }

    impl_as_any!();
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let mut b = self.pipe.buffer.lock();
        if self.is_writer {
            b.writers = b.writers.saturating_sub(1);
        } else {
            b.readers = b.readers.saturating_sub(1);
        }
    }
}

/// Set a pipe's capacity (`F_SETPIPE_SZ`).
pub fn set_capacity(end: &PipeEnd, size: usize) -> usize {
    let size = size.clamp(4096, 1024 * 1024);
    end.pipe.capacity.store(size, Ordering::Relaxed);
    size
}

pub fn capacity(end: &PipeEnd) -> usize {
    end.pipe.capacity.load(Ordering::Relaxed)
}

/// A bidirectional endpoint built from two pipe halves, used to back
/// `socketpair`. Reads drain `read_end`; writes go to `write_end`.
pub struct Duplex {
    ino: u64,
    read_end: Arc<PipeEnd>,
    write_end: Arc<PipeEnd>,
}

/// Pair a read end with a write end into one bidirectional inode.
pub fn duplex(read_end: Arc<PipeEnd>, write_end: Arc<PipeEnd>) -> Arc<Duplex> {
    Arc::new(Duplex {
        ino: next_ino(),
        read_end,
        write_end,
    })
}

impl Inode for Duplex {
    fn kind(&self) -> InodeKind {
        InodeKind::Socket
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o600
    }
    fn size(&self) -> usize {
        self.read_end.size()
    }

    fn read(&self, offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        self.read_end.read(offset, buf, nonblock)
    }

    fn write(&self, offset: usize, buf: &[u8], nonblock: bool) -> Result<usize> {
        self.write_end.write(offset, buf, nonblock)
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.read_end.read(0, buf, true)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        self.write_end.write(0, buf, true)
    }

    fn poll_readable(&self) -> bool {
        self.read_end.poll_readable()
    }

    fn poll_writable(&self) -> bool {
        self.write_end.poll_writable()
    }

    fn poll_hangup(&self) -> bool {
        self.read_end.poll_hangup()
    }

    impl_as_any!();
}

/// A named FIFO in the filesystem. Opening it yields pipe-like behaviour; we
/// back it with a single shared buffer regardless of how many openers there are,
/// which is enough for the `mkfifo` calls programs occasionally make.
pub struct FifoInode {
    ino: u64,
    mode: AtomicU32,
    pipe: Arc<Pipe>,
}

impl FifoInode {
    pub fn new(mode: u32) -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            mode: AtomicU32::new(mode & 0o7777),
            pipe: Pipe::new(),
        })
    }
}

impl Inode for FifoInode {
    fn kind(&self) -> InodeKind {
        InodeKind::Fifo
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        self.mode.load(Ordering::Relaxed)
    }
    fn set_mode(&self, mode: u32) {
        self.mode.store(mode & 0o7777, Ordering::Relaxed);
    }
    fn size(&self) -> usize {
        self.pipe.buffer.lock().data.len()
    }

    fn read(&self, _offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        loop {
            {
                let mut b = self.pipe.buffer.lock();
                if !b.data.is_empty() {
                    let n = buf.len().min(b.data.len());
                    for i in 0..n {
                        buf[i] = b.data.pop_front().unwrap();
                    }
                    return Ok(n);
                }
            }
            if nonblock {
                bail!(EAGAIN);
            }
            crate::task::yield_now();
            if crate::task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    fn write(&self, _offset: usize, buf: &[u8], _nonblock: bool) -> Result<usize> {
        self.pipe.buffer.lock().data.extend(buf);
        Ok(buf.len())
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.read(0, buf, true)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        self.write(0, buf, true)
    }

    fn poll_readable(&self) -> bool {
        !self.pipe.buffer.lock().data.is_empty()
    }

    impl_as_any!();
}
