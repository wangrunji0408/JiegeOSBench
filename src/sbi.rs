use core::arch::asm;

fn call(eid: usize, fid: usize, arg0: usize) -> isize {
    let mut error: isize;
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inlateout("a0") arg0 => error,
            options(nostack),
        );
    }
    error
}

pub fn set_timer(next: u64) {
    call(0, 0, next as usize);
}

pub fn shutdown() -> ! {
    call(8, 0, 0);
    loop {
        crate::arch::wait_for_interrupt();
    }
}
