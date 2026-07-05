//! Time: QEMU virt timebase is 10 MHz (100 ns per tick).
use core::arch::asm;

const TICKS_PER_SEC: u64 = 10_000_000;
const NS_PER_TICK: u64 = 100;
// pretend the machine booted at this unix time (2026-07-05 00:00:00 UTC)
const BOOT_UNIX: u64 = 1_783_209_600;

pub fn ticks() -> u64 {
    let t: u64;
    unsafe { asm!("rdtime {}", out(reg) t) };
    t
}

pub fn now_ns() -> u64 {
    ticks() * NS_PER_TICK
}

pub fn uptime_seconds() -> u64 {
    ticks() / TICKS_PER_SEC
}

pub fn unix_seconds() -> u64 {
    BOOT_UNIX + uptime_seconds()
}

pub fn unix_ns() -> u64 {
    BOOT_UNIX * 1_000_000_000 + now_ns()
}

/// Busy-wait until at least `ns` nanoseconds pass, polling the network.
pub fn spin_sleep_ns(ns: u64) {
    let end = now_ns() + ns;
    while now_ns() < end {
        crate::net::poll();
        core::hint::spin_loop();
    }
}
