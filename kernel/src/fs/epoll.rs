//! epoll.
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use super::file::{File, FileOps};
use crate::abi::*;
use crate::sync::SpinLock;
use crate::task::wait::WaitQueue;
use crate::task::{current, sched, signal};

struct EpollItem {
    file: Arc<File>,
    events: u32,
    data: u64,
    /// Readiness reported last time (for edge-triggered items).
    last: u32,
    disabled: bool,
}

pub struct Epoll {
    items: SpinLock<BTreeMap<i32, EpollItem>>,
    pub wq: WaitQueue,
}

impl Epoll {
    pub fn new() -> Arc<Epoll> {
        Arc::new(Epoll { items: SpinLock::new(BTreeMap::new()), wq: WaitQueue::new() })
    }

    pub fn ctl(&self, op: i32, fd: i32, file: Option<Arc<File>>, ev: EpollEvent) -> Result<(), i32> {
        let mut items = self.items.lock();
        match op {
            EPOLL_CTL_ADD => {
                if items.contains_key(&fd) {
                    return Err(EEXIST);
                }
                let file = file.ok_or(EBADF)?;
                items.insert(fd, EpollItem { file, events: ev.events, data: ev.data, last: 0, disabled: false });
            }
            EPOLL_CTL_MOD => {
                let it = items.get_mut(&fd).ok_or(ENOENT)?;
                it.events = ev.events;
                it.data = ev.data;
                it.disabled = false;
                it.last = 0;
            }
            EPOLL_CTL_DEL => {
                items.remove(&fd).ok_or(ENOENT)?;
            }
            _ => return Err(EINVAL),
        }
        Ok(())
    }

    /// Remove any registration for a file being closed by the process.
    pub fn forget_file(&self, file: &Arc<File>) {
        self.items.lock().retain(|_, it| !Arc::ptr_eq(&it.file, file));
    }

    fn scan(&self, out: &mut Vec<EpollEvent>, max: usize) {
        let mut items = self.items.lock();
        for it in items.values_mut() {
            if out.len() >= max {
                break;
            }
            if it.disabled {
                continue;
            }
            let ready = it.file.poll();
            let interest = it.events | POLLERR | POLLHUP;
            let revents = ready & interest & 0xffff;

            if report {
                out.push(EpollEvent { events: revents, _pad: 0, data: it.data });
                if it.events & EPOLLONESHOT != 0 {
                    it.disabled = true;
                }
            }
        }
    }

    pub fn wait(&self, max: usize, deadline: Option<u64>) -> Result<Vec<EpollEvent>, i32> {
        let mut out = Vec::new();
        loop {
            // Poll the network stack so readiness is current.
            crate::net::poll();
            self.scan(&mut out, max);
            if !out.is_empty() {
                return Ok(out);
            }
            if let Some(d) = deadline {
                if crate::time::monotonic_ns() >= d {
                    return Ok(out);
                }
            }
            let cur = current();
            if signal::has_deliverable(&cur) {
                return Err(EINTR);
            }
            // register on all wait queues
            let files: Vec<Arc<File>> = self.items.lock().values().filter(|it| !it.disabled).map(|it| it.file.clone()).collect();
            for f in &files {
                if let Some(wq) = f.ops.wait_queue() {
                    wq.add(&cur);
                }
            }
            self.wq.add(&cur);
            if let Some(d) = deadline {
                crate::time::add_sleeper(&cur, d);
            }
            sched::block_current();
            if deadline.is_some() {
                crate::time::remove_sleeper(&cur);
            }
            self.wq.remove(&cur);
            for f in &files {
                if let Some(wq) = f.ops.wait_queue() {
                    wq.remove(&cur);
                }
            }
        }
    }
}

impl FileOps for Epoll {
    fn poll(&self) -> u32 {
        let mut v = Vec::new();
        self.scan(&mut v, 1);
        if v.is_empty() {
            0
        } else {
            POLLIN
        }
    }
    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&self.wq)
    }
    fn stat(&self) -> Result<Stat, i32> {
        Ok(Stat { st_mode: S_IFREG | 0o600, st_nlink: 1, ..Stat::default() })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
