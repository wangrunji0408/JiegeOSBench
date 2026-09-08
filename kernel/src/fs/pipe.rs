//! Anonymous pipes.

use crate::errno::*;
use crate::sync::SpinLock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

pub const PIPE_BUF: usize = 4096;
const PIPE_CAPACITY: usize = 64 * 1024;

pub struct Pipe {
    pub buf: SpinLock<VecDeque<u8>>,
    pub readers: SpinLock<usize>,
    pub writers: SpinLock<usize>,
}

impl Pipe {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: SpinLock::new(VecDeque::with_capacity(PIPE_CAPACITY)),
            readers: SpinLock::new(1),
            writers: SpinLock::new(1),
        })
    }

    pub fn read(&self, out: &mut [u8]) -> Result<usize, Errno> {
        let mut b = self.buf.lock();
        if b.is_empty() {
            if *self.writers.lock() == 0 {
                return Ok(0); // EOF
            }
            return Err(EAGAIN);
        }
        let n = core::cmp::min(out.len(), b.len());
        for (i, x) in b.drain(..n).enumerate() {
            out[i] = x;
        }
        Ok(n)
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, Errno> {
        if *self.readers.lock() == 0 {
            return Err(EPIPE);
        }
        let mut b = self.buf.lock();
        let space = PIPE_CAPACITY.saturating_sub(b.len());
        if space == 0 {
            return Err(EAGAIN);
        }
        let n = core::cmp::min(data.len(), space);
        b.extend(&data[..n]);
        Ok(n)
    }

    pub fn readable(&self) -> bool {
        !self.buf.lock().is_empty() || *self.writers.lock() == 0
    }

    pub fn writable(&self) -> bool {
        self.buf.lock().len() < PIPE_CAPACITY
    }
}
