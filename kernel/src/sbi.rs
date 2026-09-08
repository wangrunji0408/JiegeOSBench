//! SBI (legacy v0.1) calls into OpenSBI running in M-mode.

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> (usize, usize) {
    let (mut a0, mut a1, mut a2, mut a6, mut a7);
    a0 = arg0;
    a1 = arg1;
    a2 = arg2;
    a6 = fid;
    a7 = eid;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            in("a2") a2,
            inlateout("a6") a6,
            inlateout("a7") a7,
            options(nostack)
        );
    }
    (a0, a1)
}

pub const SBI_CONSOLE_PUTCHAR: usize = 1;
pub const SBI_CONSOLE_GETCHAR: usize = 2;
pub const SBI_SET_TIMER: usize = 0;
pub const SBI_SHUTDOWN: usize = 8;

pub fn console_putchar(c: u8) {
    sbi_call(SBI_CONSOLE_PUTCHAR, 0, c as usize, 0, 0);
}

pub fn console_getchar() -> Option<u8> {
    let (r, _) = sbi_call(SBI_CONSOLE_GETCHAR, 0, 0, 0, 0);
    if r == usize::MAX || r > 255 {
        None
    } else {
        Some(r as u8)
    }
}

/// Program the next S-mode timer interrupt at absolute time `t` (10 MHz counter).
pub fn set_timer(t: u64) {
    sbi_call(SBI_SET_TIMER, 0, t as usize, 0, 0);
}

pub fn shutdown(failure: bool) -> ! {
    sbi_call(SBI_SHUTDOWN, 0, failure as usize, 0, 0);
    loop {}
}
