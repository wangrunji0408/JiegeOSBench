#![no_std]
#![no_main]
#![feature(panic_info_message)]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::kprintln!("[PANIC] {}", info);
    crate::kprintln!("[PANIC] halting");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    crate::kprintln!("[PANIC] kernel heap allocation failure: {:?}", layout);
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
