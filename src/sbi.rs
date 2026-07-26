//! Minimal SBI (Supervisor Binary Interface) calls to the firmware (OpenSBI).

use core::arch::asm;

#[inline(always)]
fn sbi_call(eid: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (usize, usize) {
    let (error, value);
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0 => error,
            inlateout("a1") a1 => value,
            in("a2") a2,
            in("a6") fid,
            in("a7") eid,
            options(nostack),
        );
    }
    (error, value)
}

const EID_TIME: usize = 0x5449_4D45;
const EID_SRST: usize = 0x5352_5354;
const EID_DBCN: usize = 0x4442_434E;
const LEGACY_PUTCHAR: usize = 0x01;
const LEGACY_GETCHAR: usize = 0x02;

/// Write a single byte to the SBI debug console.
pub fn console_putchar(c: u8) {
    sbi_call(LEGACY_PUTCHAR, 0, c as usize, 0, 0);
}

/// Read a byte from the SBI debug console, or `None` if nothing is pending.
pub fn console_getchar() -> Option<u8> {
    let (ret, _) = sbi_call(LEGACY_GETCHAR, 0, 0, 0, 0);
    if ret == usize::MAX {
        None
    } else {
        Some(ret as u8)
    }
}

/// Write a buffer via the DBCN extension (much faster than byte-at-a-time).
pub fn console_write(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return true;
    }
    let paddr = buf.as_ptr() as usize;
    let (err, _) = sbi_call(EID_DBCN, 0, buf.len(), paddr, 0);
    err == 0
}

/// Program the next timer interrupt (absolute `mtime` value).
pub fn set_timer(stime: u64) {
    sbi_call(EID_TIME, 0, stime as usize, 0, 0);
}

/// Shut the machine down.
pub fn shutdown(failure: bool) -> ! {
    let reason = if failure { 1 } else { 0 };
    sbi_call(EID_SRST, 0, 0, reason, 0);
    // Fall back to the legacy shutdown call.
    sbi_call(0x08, 0, 0, 0, 0);
    loop {
        unsafe { asm!("wfi") };
    }
}
