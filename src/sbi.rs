//! SBI 调用（console 输出、关机、定时器）

use core::arch::asm;

const SBI_SET_TIMER: usize = 0;
const SBI_CONSOLE_PUTCHAR: usize = 1;
const SBI_CONSOLE_GETCHAR: usize = 2;
const SBI_SRST: usize = 0x53525354;

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 => ret,
            in("a1") arg1,
            in("a2") arg2,
            in("a6") fid,
            in("a7") eid,
            options(nostack)
        );
    }
    ret
}

pub fn console_putchar(c: usize) {
    sbi_call(SBI_CONSOLE_PUTCHAR, 0, c, 0, 0);
}

/// 非阻塞读取一个字符，没有则返回 None
pub fn console_getchar() -> Option<u8> {
    let ret = sbi_call(SBI_CONSOLE_GETCHAR, 0, 0, 0, 0);
    if ret == usize::MAX {
        None
    } else {
        Some(ret as u8)
    }
}

pub fn set_timer(stime: u64) {
    sbi_call(SBI_SET_TIMER, 0, stime as usize, 0, 0);
}

pub fn shutdown() -> ! {
    // SRST extension: fid 0 = shutdown
    sbi_call(SBI_SRST, 0, 0, 0, 0);
    loop {
        unsafe { asm!("wfi") };
    }
}
