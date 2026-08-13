//! Minimal SBI (RISC-V Supervisor Binary Interface) wrapper for OpenSBI.

#[inline]
pub fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (usize, usize) {
    let mut error;
    let mut value;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0 => error,
            inlateout("a1") a1 => value,
            in("a2") a2,
            in("a6") fid,
            in("a7") eid,
        );
    }
    (error, value)
}

const EXT_TIME: usize = 0x54494D45;
const EXT_SRST: usize = 0x53525354;

/// Timebase frequency on QEMU `virt` (10 MHz).
pub const TIMEBASE_HZ: u64 = 10_000_000;

/// Schedule a one-shot timer interrupt `ns` nanoseconds from now
/// (converted to platform ticks internally).
pub fn set_timer(ns: u64) {
    let ticks = ns * TIMEBASE_HZ / 1_000_000_000;
    sbi_call(EXT_TIME, 0, ticks as usize, 0, 0);
}

/// Read the current time as nanoseconds since boot via the RISC-V `time` CSR.
pub fn get_time() -> u64 {
    let ticks: u64;
    unsafe { core::arch::asm!("rdtime {}", out(reg) ticks) };
    ticks * 1_000_000_000 / TIMEBASE_HZ
}

pub fn shutdown() -> ! {
    // SRST extension, system reset type SHUTDOWN (0)
    sbi_call(EXT_SRST, 0, 0, 0, 0);
    // Fallback: legacy shutdown
    unsafe { core::arch::asm!("ecall", in("a7") 0x8, in("a6") 0x0) };
    loop {}
}

/// Initialize the timer: a one-shot schedule of the first tick.
pub fn init_timer() {
    // 10ms tick for now
    set_timer(get_time() + 10_000_000);
}
