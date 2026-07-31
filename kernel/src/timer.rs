//! Timer: SBI set_timer based; periodic 10 ms ticks + one-shot timer list.

use crate::sbi;
use core::arch::asm;

pub static mut TIMEBASE: u64 = 10_000_000;

pub static mut TICKS: u64 = 0; // ms since boot
static mut TICK_MS: u64 = 10;

#[inline]
pub fn rdtime() -> u64 {
    let mut t: u64;
    unsafe {
        asm!("rdtime {0}", out(reg) t, options(nostack));
    }
    t
}

pub fn init(timebase: u64) {
    unsafe {
        TIMEBASE = timebase;
    }
    let now = rdtime();
    sbi::set_timer(now + ms_to_ticks(TICK_MS));
}

pub fn ms_to_ticks(ms: u64) -> u64 {
    unsafe { ms * TIMEBASE / 1000 }
}

pub fn ticks_to_ms(ticks: u64) -> u64 {
    unsafe { ticks * 1000 / TIMEBASE }
}

pub fn now_ms() -> u64 {
    unsafe { TICKS }
}

/// Advance time; returns true if a full tick passed.
pub fn on_timer_interrupt() {
    unsafe {
        TICKS += TICK_MS;
    }
    crate::timer_wheel::on_tick();
    let now = rdtime();
    sbi::set_timer(now + ms_to_ticks(TICK_MS));
}
