//! Time keeping and timer interrupts.

use crate::arch;
use crate::sbi;
use core::sync::atomic::{AtomicU64, Ordering};

/// The QEMU `virt` machine's `time` CSR runs at 10 MHz.
pub const TIMEBASE_FREQ: u64 = 10_000_000;
/// Scheduling quantum: 10 ms.
pub const TICK_HZ: u64 = 100;
const TICK_INTERVAL: u64 = TIMEBASE_FREQ / TICK_HZ;

static TICKS: AtomicU64 = AtomicU64::new(0);
/// Wall clock offset: seconds to add to the monotonic clock to get real time.
/// QEMU gives us no RTC via SBI, so start at a plausible fixed date — nginx only
/// needs a monotonically increasing clock and sane log timestamps.
static BOOT_UNIX_TIME: AtomicU64 = AtomicU64::new(1_774_000_000);

pub fn init() {
    schedule_next();
}

fn schedule_next() {
    sbi::set_timer(arch::time() + TICK_INTERVAL);
}

pub fn on_timer_tick() {
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    schedule_next();
    crate::task::on_tick();
    crate::syscall::misc_ops::check_itimers();

    // Periodic health line, for diagnosing a stalled network or scheduler.
    if crate::console::trace_enabled() && ticks % (TICK_HZ * 4) == 0 {
        let (rx, tx, rx_drop, tx_drop) = crate::drivers::virtio_net::stats();
        let (posted, pending, ready) = crate::drivers::virtio_net::rx_debug();
        crate::println!(
            "\x1b[90m[health]\x1b[0m t={}s rx={} tx={} drop={}/{} ring={}/{}/{} irq={} poll={} sw={} tasks={}",
            ticks / TICK_HZ,
            rx,
            tx,
            rx_drop,
            tx_drop,
            posted,
            pending,
            ready,
            crate::drivers::virtio_net::IRQ_COUNT.load(Ordering::Relaxed),
            crate::net::stack::POLL_COUNT.load(Ordering::Relaxed),
            crate::task::context_switches(),
            crate::task::all_tasks().len(),
        );
    }
}

/// Nanoseconds since boot.
#[inline]
pub fn monotonic_ns() -> u64 {
    // 10 MHz -> 100 ns per tick.
    arch::time().wrapping_mul(100)
}

/// Microseconds since boot.
#[inline]
pub fn monotonic_us() -> u64 {
    arch::time() / 10
}

/// Milliseconds since boot.
#[inline]
pub fn monotonic_ms() -> u64 {
    arch::time() / 10_000
}

/// (seconds, nanoseconds) since boot.
pub fn monotonic() -> (u64, u64) {
    let ns = monotonic_ns();
    (ns / 1_000_000_000, ns % 1_000_000_000)
}

/// (seconds, nanoseconds) of wall clock time.
pub fn realtime() -> (u64, u64) {
    let (s, ns) = monotonic();
    (s + BOOT_UNIX_TIME.load(Ordering::Relaxed), ns)
}

/// Set the wall clock (used by `clock_settime`).
pub fn set_realtime(secs: u64) {
    let (mono, _) = monotonic();
    BOOT_UNIX_TIME.store(secs.saturating_sub(mono), Ordering::Relaxed);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Busy-wait for the given number of microseconds.
pub fn delay_us(us: u64) {
    let target = arch::time() + us * (TIMEBASE_FREQ / 1_000_000);
    while arch::time() < target {
        core::hint::spin_loop();
    }
}
