//! JiegeOS — a RISC-V OS kernel in Rust that runs the official nginx binary.
#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

#[macro_use]
mod console;
mod sbi;

use core::arch::global_asm;
use core::panic::PanicInfo;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    la sp, __boot_stack_top
    j rust_main

    .section .bss.stack
    .globl __boot_stack
__boot_stack:
    .space 1024 * 64
    .globl __boot_stack_top
__boot_stack_top:
"#
);

#[no_mangle]
extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    clear_bss();
    println!("[jiege-os] booting on hart {} dtb={:#x}", hartid, dtb);
    println!("[jiege-os] hello from Rust RISC-V kernel!");
    sbi::shutdown();
}

fn clear_bss() {
    extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of_mut!(__bss_start);
        let end = core::ptr::addr_of_mut!(__bss_end);
        let len = end as usize - start as usize;
        // The boot stack lives in .bss and is in use — it is at the start of
        // .bss (".bss.stack" placed first), so skip it.
        let stack_size = 1024 * 64;
        core::ptr::write_bytes(start.add(stack_size), 0, len - stack_size);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n[kernel PANIC] {}", info);
    sbi::shutdown()
}
