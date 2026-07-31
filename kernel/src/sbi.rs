//! SBI (Supervisor Binary Interface) calls into OpenSBI (M-mode firmware).

use core::arch::asm;

#[inline(always)]
unsafe fn sbi_call(which: usize, arg0: usize, arg1: usize, arg2: usize) -> (usize, usize) {
    let mut ret0: usize;
    let mut ret1: usize;
    asm!(
        "ecall",
        inlateout("a0") arg0 => ret0,
        inlateout("a1") arg1 => ret1,
        in("a7") which,
        options(nostack)
    );
    (ret0, ret1)
}

/// Legacy console putchar.
pub fn console_putchar(c: u8) {
    unsafe {
        sbi_call(1, c as usize, 0, 0);
    }
}

/// Legacy console getchar; returns -1 if no char.
pub fn console_getchar() -> isize {
    let (ret, _) = unsafe { sbi_call(2, 0, 0, 0) };
    ret as isize
}

/// Legacy set timer (absolute time in ticks).
pub fn set_timer(stime_value: u64) {
    unsafe {
        sbi_call(0, stime_value as usize, 0, 0);
    }
}

/// Legacy shutdown.
pub fn shutdown() -> ! {
    unsafe {
        sbi_call(8, 0, 0, 0);
    }
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
