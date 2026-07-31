#![no_std]
#![no_main]
#![feature(panic_info_message)]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::console::kprintln!("[PANIC] {}", info);
    crate::console::kprintln!("[PANIC] halting");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    crate::console::kprintln!("[PANIC] kernel heap allocation failure: {:?}", layout);
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
