//! Futexes.
//!
//! musl's mutexes, condition variables and thread joins all funnel through
//! `futex`, so nginx needs `FUTEX_WAIT` / `FUTEX_WAKE` to work. We key wait
//! queues on the user virtual address; since all our processes share one page
//! table view and we never have two distinct mappings at one address, that is
//! sufficient (private futexes are per-address-space anyway, and we only run one
//! address space per process).

use super::sched;
use super::task::Task;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// Futex operation codes.
pub const FUTEX_WAIT: usize = 0;
pub const FUTEX_WAKE: usize = 1;
pub const FUTEX_FD: usize = 2;
pub const FUTEX_REQUEUE: usize = 3;
pub const FUTEX_CMP_REQUEUE: usize = 4;
pub const FUTEX_WAKE_OP: usize = 5;
pub const FUTEX_LOCK_PI: usize = 6;
pub const FUTEX_UNLOCK_PI: usize = 7;
pub const FUTEX_TRYLOCK_PI: usize = 8;
pub const FUTEX_WAIT_BITSET: usize = 9;
pub const FUTEX_WAKE_BITSET: usize = 10;

pub const FUTEX_PRIVATE_FLAG: usize = 128;
pub const FUTEX_CLOCK_REALTIME: usize = 256;
pub const FUTEX_CMD_MASK: usize = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

/// A waiter parked on a futex.
struct Waiter {
    task: Arc<Task>,
    /// The bitset from `FUTEX_WAIT_BITSET`; `!0` for plain waits.
    bitset: u32,
    /// Set by the waker so the waiter knows it was woken rather than timing out.
    woken: Arc<Mutex<bool>>,
}

/// Wait queues keyed by user address.
static QUEUES: Mutex<BTreeMap<usize, Vec<Waiter>>> = Mutex::new(BTreeMap::new());

/// Park the current task on `addr`. `deadline_ms` of `None` means wait forever.
///
/// Returns `Ok(())` if woken, `Err(ETIMEDOUT)` on timeout, `Err(EINTR)` if a
/// signal arrived.
pub fn wait(addr: usize, bitset: u32, deadline_ms: Option<u64>) -> crate::fs::Result<()> {
    let task = sched::current();
    let woken = Arc::new(Mutex::new(false));
    QUEUES.lock().entry(addr).or_default().push(Waiter {
        task: task.clone(),
        bitset,
        woken: woken.clone(),
    });

    loop {
        if *woken.lock() {
            return Ok(());
        }
        if let Some(deadline) = deadline_ms {
            if crate::time::monotonic_ms() >= deadline {
                remove_waiter(addr, task.tid);
                crate::bail!(ETIMEDOUT);
            }
        }
        if sched::has_pending_signal() {
            remove_waiter(addr, task.tid);
            crate::bail!(EINTR);
        }
        // Yield rather than fully blocking: with one hart, a `FUTEX_WAKE` from
        // another task can only run if we give up the CPU, and yielding keeps
        // the timeout and signal checks simple.
        sched::yield_now();
    }
}

fn remove_waiter(addr: usize, tid: usize) {
    let mut queues = QUEUES.lock();
    if let Some(list) = queues.get_mut(&addr) {
        list.retain(|w| w.task.tid != tid);
        if list.is_empty() {
            queues.remove(&addr);
        }
    }
}

/// Wake up to `count` waiters on `addr`. Returns how many were woken.
pub fn wake(addr: usize, count: usize) -> usize {
    wake_bitset(addr, count, u32::MAX)
}

pub fn wake_bitset(addr: usize, count: usize, bitset: u32) -> usize {
    let mut queues = QUEUES.lock();
    let Some(list) = queues.get_mut(&addr) else {
        return 0;
    };
    let mut woken = 0;
    let mut i = 0;
    while i < list.len() && woken < count {
        if list[i].bitset & bitset != 0 {
            let waiter = list.remove(i);
            *waiter.woken.lock() = true;
            sched::enqueue(waiter.task);
            woken += 1;
        } else {
            i += 1;
        }
    }
    if list.is_empty() {
        queues.remove(&addr);
    }
    woken
}

/// `FUTEX_REQUEUE` / `FUTEX_CMP_REQUEUE`: wake `wake_count` waiters on `addr`
/// and move up to `requeue_count` of the rest to `addr2`.
pub fn requeue(addr: usize, addr2: usize, wake_count: usize, requeue_count: usize) -> usize {
    let woken = wake(addr, wake_count);
    let mut queues = QUEUES.lock();
    let Some(mut list) = queues.remove(&addr) else {
        return woken;
    };
    let n = requeue_count.min(list.len());
    let moved: Vec<Waiter> = list.drain(..n).collect();
    if !list.is_empty() {
        queues.insert(addr, list);
    }
    if !moved.is_empty() {
        queues.entry(addr2).or_default().extend(moved);
    }
    woken + n
}

/// Number of tasks waiting on `addr`, for diagnostics.
pub fn waiter_count(addr: usize) -> usize {
    QUEUES.lock().get(&addr).map_or(0, |l| l.len())
}
