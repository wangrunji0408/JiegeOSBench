//! SBI (Supervisor Binary Interface) 调用封装

const SBI_SET_TIMER: usize = 0x54494D45; // "TIME" 扩展
const SBI_SYSTEM_RESET: usize = 0x53525354; // "SRST" 扩展
const SBI_LEGACY_SHUTDOWN: usize = 0x08;

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (usize, usize, usize) {
    let (error, value, _t);
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0 => error,
            inlateout("a1") a1 => value,
            inlateout("a2") a2 => _t,
            in("a3") 0usize,
            in("a6") fid,
            in("a7") eid,
        );
    }
    (error, value, _t)
}

/// 设置下一个 supervisor timer 中断的绝对时刻（mtime ticks）
pub fn set_timer(stime: u64) {
    let _ = sbi_call(SBI_SET_TIMER, 0, stime as usize, 0, 0);
}

/// 关机（正常退出）
pub fn shutdown() -> ! {
    // SRST 扩展：type=0(shutdown), reason=0
    let _ = sbi_call(SBI_SYSTEM_RESET, 0, 0, 0, 0);
    // 兜底：legacy shutdown
    let _ = sbi_call(SBI_LEGACY_SHUTDOWN, 0, 0, 0, 0);
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}
