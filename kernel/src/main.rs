#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![allow(dead_code)]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

#[macro_use]
mod uart;
mod config;
mod frame;
mod heap;
mod memory;
mod page_table;
mod sbi;

global_asm!(
    ".section .text.entry",
    ".globl _start",
    "_start:",
    "   la sp, boot_stack_top",
    "   call rust_main",
    "1: wfi",
    "   j 1b",
    ".section .bss.stack",
    ".globl boot_stack_lower",
    "boot_stack_lower:",
    "   .space 4096 * 64",
    ".globl boot_stack_top",
    "boot_stack_top:",
);

fn clear_bss() {
    extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as usize;
        let end = ebss as usize;
        let mut p = start;
        while p + 8 <= end {
            (p as *mut u64).write_volatile(0);
            p += 8;
        }
        while p < end {
            (p as *mut u8).write_volatile(0);
            p += 1;
        }
    }
}

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    clear_bss();
    uart::init();
    println!();
    println!("[kernel] booted: hartid={} dtb={:#x}", hartid, dtb);
    memory::init();
    println!("[kernel] paging enabled, free frames: {}", frame::free_count());

    // Smoke test the heap.
    let mut v = alloc::vec::Vec::new();
    for i in 0..1000 {
        v.push(i);
    }
    println!("[kernel] heap ok, vec sum = {}", v.iter().sum::<i32>());

    println!("[kernel] init complete");
    sbi::shutdown();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[kernel] PANIC: {}", info);
    sbi::shutdown();
}
