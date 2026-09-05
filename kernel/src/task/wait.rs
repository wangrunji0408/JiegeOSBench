//! Wait queues and blocking helpers.
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

use super::{current, sched, signal, Task};
use crate::abi::{EAGAIN, EINTR};
use crate::sync::SpinLock;

pub struct WaitQueue {
    waiters: SpinLock<Vec<Weak<Task>>>,
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueue {
    pub const fn new() -> Self {
        WaitQueue { waiters: SpinLock::new(Vec::new()) }
    }

    pub fn add(&self, task: &Arc<Task>) {
        let mut w = self.waiters.lock();
        if !w.iter().any(|t| t.as_ptr() == Arc::as_ptr(task)) {
            w.push(Arc::downgrade(task));
        }
    }

    pub fn remove(&self, task: &Arc<Task>) {
        let mut w = self.waiters.lock();
        w.retain(|t| t.as_ptr() != Arc::as_ptr(task));
    }

    pub fn wake_all(&self) {
        let list: Vec<Weak<Task>> = core::mem::take(&mut *self.waiters.lock());
        for w in list {
            if let Some(t) = w.upgrade() {
                sched::make_runnable(&t);
            }
        }
    }

    pub fn wake_one(&self) {
        let t = {
            let mut w = self.waiters.lock();
            if w.is_empty() {
                None
            } else {
                Some(w.remove(0))
            }
        };
        if let Some(t) = t.and_then(|w| w.upgrade()) {
            sched::make_runnable(&t);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.lock().is_empty()
    }

    /// Block the current task on this queue (spurious wakeups possible).
    pub fn wait(&self) {
        let cur = current();
        self.add(&cur);
        sched::block_current();
        self.remove(&cur);
    }

    /// Block on this queue until woken or `deadline` (monotonic ns) passes.
    /// Returns true if the deadline passed.
    pub fn wait_until(&self, deadline: Option<u64>) -> bool {
        let cur = current();
        if let Some(d) = deadline {
            if crate::time::monotonic_ns() >= d {
                return true;
            }
            crate::time::add_sleeper(&cur, d);
        }
        self.add(&cur);
        sched::block_current();
        self.remove(&cur);
        if let Some(d) = deadline {
            crate::time::remove_sleeper(&cur);
            return crate::time::monotonic_ns() >= d;
        }
        false
    }
}

/// Repeatedly evaluate `try_op`; if it returns Err(EAGAIN) and blocking is
/// allowed, sleep on `wq` and retry. Signal arrival yields EINTR.
pub fn block_on<T, F>(wqs: &[&WaitQueue], nonblock: bool, mut try_op: F) -> Result<T, i32>
where
    F: FnMut() -> Result<T, i32>,
{
    loop {
        match try_op() {
            Err(EAGAIN) if !nonblock => {
                let cur = current();
                if signal::has_deliverable(&cur) {
                    return Err(EINTR);
                }
                for wq in wqs {
                    wq.add(&cur);
                }
                sched::block_current();
                for wq in wqs {
                    wq.remove(&cur);
                }
            }
            r => return r,
        }
    }
}

/// Same as `block_on` but with a deadline (monotonic ns). Returns EAGAIN on timeout.
pub fn block_on_until<T, F>(wqs: &[&WaitQueue], deadline: Option<u64>, mut try_op: F) -> Result<T, i32>
where
    F: FnMut() -> Result<T, i32>,
{
    loop {
        match try_op() {
            Err(EAGAIN) => {
                let cur = current();
                if signal::has_deliverable(&cur) {
                    return Err(EINTR);
                }
                if let Some(d) = deadline {
                    if crate::time::monotonic_ns() >= d {
                        return Err(EAGAIN);
                    }
                    crate::time::add_sleeper(&cur, d);
                }
                for wq in wqs {
                    wq.add(&cur);
                }
                sched::block_current();
                for wq in wqs {
                    wq.remove(&cur);
                }
                if deadline.is_some() {
                    crate::time::remove_sleeper(&cur);
                }
            }
            r => return r,
        }
    }
}
