//! Minimal SBI (v0.2+) calls.
use core::arch::asm;

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (isize, usize) {
    let (err, val): (isize, usize);
    unsafe {
        asm!("ecall",
            inlateout("a0") a0 => err,
            inlateout("a1") a1 => val,
            in("a2") a2,
            in("a6") fid,
            in("a7") eid,
        );
    }
    (err, val)
}

pub fn console_putchar(c: u8) {
    // legacy extension 0x01
    sbi_call(0x01, 0, c as usize, 0, 0);
}

pub fn set_timer(stime: u64) {
    sbi_call(0x54494D45, 0, stime as usize, 0, 0);
}

pub fn shutdown() -> ! {
    // SRST extension: type 0 = shutdown, reason 0
    sbi_call(0x53525354, 0, 0, 0, 0);
    // legacy fallback
    sbi_call(0x08, 0, 0, 0, 0);
    loop {}
}
