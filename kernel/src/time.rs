use crate::config::CLOCK_FREQ;
use core::arch::asm;

// Wall-clock base so tv_sec is never 0 (nginx's ngx_time_update treats sec==0
// on the first tick as "time unchanged" and skips initializing its cached
// log-time string pointers, leading to a NULL deref).
const BASE_EPOCH: u64 = 1_704_067_200; // 2024-01-01 00:00:00 UTC

#[inline]
pub fn read_time() -> u64 {
    let t: u64;
    unsafe {
        asm!("rdtime {}", out(reg) t);
    }
    t
}

/// (seconds, nanoseconds) of wall-clock time.
pub fn now() -> (u64, u64) {
    let t = read_time();
    let sec = BASE_EPOCH + t / CLOCK_FREQ;
    let rem = t % CLOCK_FREQ;
    let nsec = rem * (1_000_000_000 / CLOCK_FREQ);
    (sec, nsec)
}

pub fn now_ms() -> u64 {
    let t = read_time();
    t / (CLOCK_FREQ / 1000)
}
