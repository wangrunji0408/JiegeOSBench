pub fn shutdown(failure: bool) -> ! {
    loop {
        unsafe {
            core::arch::asm!(
                "ecall",
                in("a0") 0usize,
                in("a1") if failure { 1usize } else { 0usize },
                in("a6") 0usize,
                in("a7") 0x53525354usize,
                options(nostack)
            );
        }
        // If SRST is unavailable, spin.
        unsafe { core::arch::asm!("wfi") };
    }
}

pub fn console_putchar(c: u8) {
    unsafe {
        core::arch::asm!("ecall", in("a0") c as usize, in("a6") 0usize, in("a7") 1usize);
    }
}
