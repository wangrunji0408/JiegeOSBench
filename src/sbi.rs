//! SBI 调用封装。我们使用 OpenSBI 提供的 SBI v0.2 EID 扩展。
//! SBI 调用约定：a6=function id (fid), a7=extension id (eid),
//! a0..a5 参数，返回值在 a0(error) a1(value)。

use core::arch::asm;

// EID
const SBI_SET_TIMER: usize = 0x00;
const SBI_CONSOLE_PUTCHAR: usize = 0x01;
const SBI_CONSOLE_GETCHAR: usize = 0x02;
const SBI_SHUTDOWN: usize = 0x08;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SbiRet {
    pub error: i64,
    pub value: i64,
}

#[inline]
fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> SbiRet {
    let mut error: usize;
    let mut value: usize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 => error,
            inlateout("a1") arg1 => value,
            in("a2") arg2,
            in("a6") fid,
            in("a7") eid,
            options(nostack, preserves_flags),
        );
    }
    SbiRet {
        error: error as i64,
        value: value as i64,
    }
}

/// 关机（实际是退出 QEMU）
pub fn shutdown() -> ! {
    sbi_call(SBI_SHUTDOWN, 0, 0, 0, 0);
    // 若 SBI 不支持，再用失败退出码
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

/// 通过 SBI 输出字符（备用，主用直接写 UART）
pub fn console_putchar(c: usize) {
    sbi_call(SBI_CONSOLE_PUTCHAR, 0, c, 0, 0);
}

pub fn console_getchar() -> Option<u8> {
    let ret = sbi_call(SBI_CONSOLE_GETCHAR, 0, 0, 0, 0);
    if ret.error == 0 && ret.value >= 0 {
        Some(ret.value as u8)
    } else {
        None
    }
}

/// 设置下次时钟中断（stimecmp 通过 SBI）
pub fn set_timer(stime: usize) {
    sbi_call(SBI_SET_TIMER, 0, stime, 0, 0);
}
