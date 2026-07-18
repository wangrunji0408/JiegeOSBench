//! A simple bidirectional in-kernel byte pipe, used for `socketpair` (the
//! master/worker control channel) and plain `pipe2`. Both ends are plain
//! `Arc<dyn File>` objects, so they survive `fork` (which clones the fd
//! table's `Arc`s) exactly like a real kernel would share the underlying
//! "open file description".

use super::File;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

pub struct PipeEnd {
    read_buf: Arc<Mutex<VecDeque<u8>>>,
    write_buf: Arc<Mutex<VecDeque<u8>>>,
}

pub fn pair() -> (PipeEnd, PipeEnd) {
    let a_to_b = Arc::new(Mutex::new(VecDeque::new()));
    let b_to_a = Arc::new(Mutex::new(VecDeque::new()));
    (
        PipeEnd {
            read_buf: b_to_a.clone(),
            write_buf: a_to_b.clone(),
        },
        PipeEnd {
            read_buf: a_to_b,
            write_buf: b_to_a,
        },
    )
}

impl File for PipeEnd {
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        let mut b = self.read_buf.lock();
        let n = buf.len().min(b.len());
        for slot in buf.iter_mut().take(n) {
            *slot = b.pop_front().unwrap();
        }
        n
    }
    fn write(&self, buf: &[u8]) -> usize {
        let mut b = self.write_buf.lock();
        b.extend(buf.iter().copied());
        buf.len()
    }
    fn poll_readable(&self) -> bool {
        !self.read_buf.lock().is_empty()
    }
    fn poll_writable(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
