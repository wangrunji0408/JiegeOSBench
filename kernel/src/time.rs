//! Time keeping, the periodic tick and sleeping tasks.
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::config::{TICK_HZ, TIMEBASE_FREQ};
use crate::sbi;
use crate::sync::SpinLock;
use crate::task::{sched, Task};
use crate::trap::csr;

static BOOT_REALTIME_NS: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);

struct Sleeper {
    deadline: u64,
    task: Weak<Task>,
}

static SLEEPERS: SpinLock<Vec<Sleeper>> = SpinLock::new(Vec::new());

const GOLDFISH_RTC: usize = 0x101000;

pub fn init() {
    // Read the goldfish RTC (ns since epoch) while paging is off.
    let lo = unsafe { ((GOLDFISH_RTC) as *const u32).read_volatile() } as u64;
    let hi = unsafe { ((GOLDFISH_RTC + 4) as *const u32).read_volatile() } as u64;
    let rtc_ns = (hi << 32) | lo;
    let mono = monotonic_ns();
    BOOT_REALTIME_NS.store(rtc_ns.saturating_sub(mono), Ordering::Relaxed);
    set_next_tick();
}

#[inline]
pub fn ticks_to_ns(t: u64) -> u64 {
    // TIMEBASE_FREQ = 10 MHz -> 100 ns per tick
    t * (1_000_000_000 / TIMEBASE_FREQ)
}

#[inline]
pub fn ns_to_ticks(ns: u64) -> u64 {
    ns / (1_000_000_000 / TIMEBASE_FREQ)
}

pub fn monotonic_ns() -> u64 {
    ticks_to_ns(csr::read_time())
}

pub fn realtime_ns() -> u64 {
    BOOT_REALTIME_NS.load(Ordering::Relaxed) + monotonic_ns()
}

pub fn set_realtime_ns(ns: u64) {
    BOOT_REALTIME_NS.store(ns.saturating_sub(monotonic_ns()), Ordering::Relaxed);
}

pub fn jiffies() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

fn set_next_tick() {
    let now = csr::read_time();
    sbi::set_timer(now + TIMEBASE_FREQ / TICK_HZ);
}

pub fn add_sleeper(task: &Arc<Task>, deadline: u64) {
    SLEEPERS.lock().push(Sleeper { deadline, task: Arc::downgrade(task) });
}

pub fn remove_sleeper(task: &Arc<Task>) {
    SLEEPERS.lock().retain(|s| s.task.as_ptr() != Arc::as_ptr(task));
}

fn wake_sleepers() {
    let now = monotonic_ns();
    let expired: Vec<Weak<Task>> = {
        let mut s = SLEEPERS.lock();
        let mut out = Vec::new();
        s.retain(|sl| {
            if sl.deadline <= now {
                out.push(sl.task.clone());
                false
            } else {
                true
            }
        });
        out
    };
    for w in expired {
        if let Some(t) = w.upgrade() {
            sched::make_runnable(&t);
        }
    }
}

/// Timer interrupt.
pub fn on_timer_irq() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    set_next_tick();
    wake_sleepers();
    crate::net::poll();
}

/// Sleep the current task until `deadline` (monotonic ns). Returns remaining ns
/// if interrupted by a signal.
pub fn sleep_until(deadline: u64) -> Result<(), u64> {
    let cur = crate::task::current();
    loop {
        let now = monotonic_ns();
        if now >= deadline {
            return Ok(());
        }
        if crate::task::signal::has_deliverable(&cur) {
            return Err(deadline - now);
        }
        add_sleeper(&cur, deadline);
        sched::block_current();
        remove_sleeper(&cur);
    }
}
