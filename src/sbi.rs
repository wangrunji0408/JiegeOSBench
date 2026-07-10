use core::arch::asm;

const EXT_SYSTEM_RESET: usize = 0x5352_5354;
const RESET_TYPE_SHUTDOWN: usize = 0;
const RESET_REASON_NONE: usize = 0;
const RESET_REASON_FAILURE: usize = 1;

#[inline]
fn call(extension: usize, function: usize, arg0: usize, arg1: usize) -> (usize, usize) {
    let mut error = arg0;
    let mut value = arg1;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") error,
            inlateout("a1") value,
            in("a6") function,
            in("a7") extension,
            options(nostack)
        );
    }
    (error, value)
}

pub fn shutdown(failed: bool) -> ! {
    let reason = if failed {
        RESET_REASON_FAILURE
    } else {
        RESET_REASON_NONE
    };
    let _ = call(EXT_SYSTEM_RESET, 0, RESET_TYPE_SHUTDOWN, reason);
    loop {
        unsafe { asm!("wfi") };
    }
}
