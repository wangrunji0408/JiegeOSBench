#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;

mod sbi;
mod console;
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
