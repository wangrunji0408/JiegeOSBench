use crate::config::CLOCK_FREQ;
use core::arch::asm;

#[inline]
pub fn read_time() -> u64 {
    let t: u64;
    unsafe {
        asm!("rdtime {}", out(reg) t);
    }
    t
}

/// (seconds, nanoseconds) since boot.
pub fn now() -> (u64, u64) {
    let t = read_time();
    let sec = t / CLOCK_FREQ;
    let rem = t % CLOCK_FREQ;
    let nsec = rem * (1_000_000_000 / CLOCK_FREQ);
    (sec, nsec)
}

pub fn now_ms() -> u64 {
    let t = read_time();
    t / (CLOCK_FREQ / 1000)
}
