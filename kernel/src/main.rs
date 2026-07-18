#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;

mod console;
mod sbi;
mod trap;
mod lang_items {
    use super::*;

    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        crate::println!("[kernel] panicked: {}", info);
        crate::sbi::shutdown(true);
    }
}

global_asm!(include_str!("entry.asm"));

const BOOT_STACK_SIZE: usize = 64 * 1024;

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    clear_bss();
    println!("[kernel] hello from riscv64 kernel!");
    println!("[kernel] boot stack size = {} KiB", BOOT_STACK_SIZE / 1024);
    trap::init();
    println!("[kernel] trap vector installed, testing ebreak...");
    unsafe {
        core::arch::asm!("ebreak");
    }
    println!("[kernel] survived ebreak, waiting for a few timer interrupts...");
    let start = riscv::register::time::read64();
    let mut ticks_seen = 0u32;
    let mut last = start;
    while ticks_seen < 3 {
        let now = riscv::register::time::read64();
        if now - last > 900_000 {
            ticks_seen += 1;
            last = now;
            println!("[kernel] ~timer tick {}", ticks_seen);
        }
        core::hint::spin_loop();
    }
    println!("[kernel] M1 trap handling verified. shutting down.");
    sbi::shutdown(false);
}

fn clear_bss() {
    unsafe extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as *const () as usize;
        let end = ebss as *const () as usize;
        core::slice::from_raw_parts_mut(start as *mut u8, end - start).fill(0);
    }
}
