//! Minimal SBI calls (legacy console + SRST shutdown).
use core::arch::asm;

#[inline]
fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (isize, usize) {
    let (error, value);
    unsafe {
        asm!(
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

/// Legacy console putchar (EID 0x01).
pub fn console_putchar(c: u8) {
    sbi_call(0x01, 0, c as usize, 0, 0);
}

/// System reset extension: shutdown.
pub fn shutdown() -> ! {
    sbi_call(0x53525354, 0, 0, 0, 0);
    loop {
        unsafe { asm!("wfi") };
    }
}
