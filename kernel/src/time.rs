//! Timekeeping: SBI timer (10 MHz on QEMU virt) plus the goldfish RTC for
//! wall-clock time.

use crate::csr;
use crate::mm::address::dev_addr;
use crate::sbi;

pub const TIMER_FREQ: u64 = 10_000_000;
/// Timer interrupt period: 1 ms.
pub const TICK_TICKS: u64 = TIMER_FREQ / 1000;

const RTC_BASE: usize = 0x0010_1000;

static mut BOOT_TIME_NS: u64 = 0;
static mut NEXT_DEADLINE: u64 = 0;
static mut TICKS: u64 = 0;

#[inline]
pub fn rdtime() -> u64 {
    csr::time() as u64
}

/// Nanoseconds since boot.
#[inline]
pub fn uptime_ns() -> u64 {
    rdtime() * 100
}

pub fn ticks() -> u64 {
    unsafe { TICKS }
}

/// Read the goldfish RTC: nanoseconds since the Unix epoch.
fn read_rtc_ns() -> Option<u64> {
    unsafe {
        let base = dev_addr(RTC_BASE) as *const u32;
        let lo = core::ptr::read_volatile(base);
        let hi = core::ptr::read_volatile(base.add(1));
        let v = ((hi as u64) << 32) | lo as u64;
        if v == 0 {
            None
        } else {
            Some(v)
        }
    }
}

pub fn init() {
    let epoch = read_rtc_ns().unwrap_or(1_700_000_000_000_000_000);
    unsafe {
        BOOT_TIME_NS = epoch.wrapping_sub(uptime_ns());
    }
    arm(TICK_TICKS);
}

pub fn arm(delta_ticks: u64) {
    let next = rdtime() + delta_ticks;
    unsafe {
        NEXT_DEADLINE = next;
    }
    sbi::set_timer(next);
}

/// Called from the timer interrupt: re-arm and count.
pub fn on_tick() {
    unsafe {
        TICKS += 1;
    }
    arm(TICK_TICKS);
    crate::task::on_timer();
}

/// CLOCK_REALTIME in nanoseconds.
pub fn realtime_ns() -> u64 {
    unsafe { BOOT_TIME_NS + uptime_ns() }
}

pub fn realtime_ms() -> u64 {
    realtime_ns() / 1_000_000
}
