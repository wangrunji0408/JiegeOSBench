use core::arch::asm;

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (usize, usize) {
    let (error, value);
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inlateout("a0") a0 => error,
            inlateout("a1") a1 => value,
            in("a2") a2,
            options(nostack),
        );
    }
    (error, value)
}

// Legacy timer extension (EID 0x00).
pub fn set_timer(time: u64) {
    sbi_call(0x54494D45, 0, time as usize, 0, 0);
}

pub fn shutdown() -> ! {
    // SRST extension: system reset.
    sbi_call(0x53525354, 0, 0, 0, 0);
    // Fallback legacy shutdown.
    sbi_call(0x08, 0, 0, 0, 0);
    loop {}
}
