//! futex: wait queues keyed by the physical address of the futex word.
use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use crate::abi::*;
use crate::mm::addrspace::AccessKind;
use crate::mm::uaccess::current_mm;
use crate::sync::SpinLock;
use crate::task::wait::WaitQueue;
use crate::task::{current, sched, signal};
use crate::time::monotonic_ns;

static QUEUES: SpinLock<BTreeMap<usize, Arc<WaitQueue>>> = SpinLock::new(BTreeMap::new());

fn key_for(uaddr: usize) -> Result<usize, i32> {
    let mm = current_mm();
    let pa = mm.lock().access(uaddr, AccessKind::Read).ok_or(EFAULT)?;
    Ok(pa)
}

fn queue(key: usize) -> Arc<WaitQueue> {
    QUEUES.lock().entry(key).or_insert_with(|| Arc::new(WaitQueue::new())).clone()
}

/// Wake up to `n` waiters on the futex at user address `uaddr` (current mm).
pub fn wake(uaddr: usize, n: usize) -> usize {
    let Ok(key) = key_for(uaddr) else { return 0 };
    let q = queue(key);
    let mut woken = 0;
    for _ in 0..n {
        if q.is_empty() {
            break;
        }
        q.wake_one();
        woken += 1;
    }
    woken
}

pub fn sys_futex(uaddr: usize, op: i32, val: u32, timeout: usize, uaddr2: usize, val3: u32) -> SysResult {
    let cmd = op & FUTEX_CMD_MASK;
    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let key = key_for(uaddr)?;
            let deadline = if timeout != 0 {
                let ts: Timespec = crate::mm::uaccess::read_val(timeout)?;
                let ns = ts.tv_sec.max(0) as u64 * 1_000_000_000 + ts.tv_nsec as u64;
                if cmd == FUTEX_WAIT_BITSET {
                    // absolute
                    if op & FUTEX_CLOCK_REALTIME != 0 {
                        Some(monotonic_ns() + ns.saturating_sub(crate::time::realtime_ns()))
                    } else {
                        Some(ns)
                    }
                } else {
                    Some(monotonic_ns() + ns)
                }
            } else {
                None
            };
            let q = queue(key);
            let cur = current();
            let cur_val: u32 = crate::mm::uaccess::read_val(uaddr)?;
            if cur_val != val {
                return Err(EAGAIN);
            }
            if signal::has_deliverable(&cur) {
                return Err(EINTR);
            }
            if let Some(d) = deadline {
                if monotonic_ns() >= d {
                    return Err(ETIMEDOUT);
                }
                crate::time::add_sleeper(&cur, d);
            }
            q.add(&cur);
            sched::block_current();
            q.remove(&cur);
            if deadline.is_some() {
                crate::time::remove_sleeper(&cur);
            }
            if signal::has_deliverable(&cur) {
                return Err(EINTR);
            }
            if let Some(d) = deadline {
                if monotonic_ns() >= d {
                    return Err(ETIMEDOUT);
                }
            }
            // Spurious wakeups are allowed by the futex contract.
            Ok(0)
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            let _ = val3;
            Ok(wake(uaddr, val as usize))
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            if cmd == FUTEX_CMP_REQUEUE {
                let cur_val: u32 = crate::mm::uaccess::read_val(uaddr)?;
                if cur_val != val3 {
                    return Err(EAGAIN);
                }
            }
            let woken = wake(uaddr, val as usize);
            // Move remaining waiters: simply wake them too (they will re-check).
            let _ = uaddr2;
            let key = key_for(uaddr)?;
            queue(key).wake_all();
            Ok(woken)
        }
        _ => Err(ENOSYS),
    }
}
