#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

#[macro_use]
mod console;
mod config;
mod drivers;
mod elf;
mod fd;
mod fs;
mod mm;
mod net;
mod sbi;
mod signal;
mod sync;
mod syscall;
mod task;
mod timer;
mod trap;

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    # a0 = hartid, a1 = dtb
    # 先清零 bss（此时还未使用栈，boot_stack 也在 bss 中）
    la t0, sbss
    la t1, ebss
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    la sp, boot_stack_top
    call rust_main

    .section .bss.stack
    .globl boot_stack
boot_stack:
    .space 4096 * 16
    .globl boot_stack_top
boot_stack_top:
"#
);

#[no_mangle]
fn rust_main(_hartid: usize, _dtb: usize) -> ! {
    println!("iJiege-k3 kernel booting...");
    mm::init_heap();
    mm::init_frame_allocator();
    trap::init();
    timer::init();
    drivers::virtio::init();
    net::init();
    fs::init();
    task::init();
    task::run_tasks();
    panic!("scheduler returned");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[KERNEL PANIC] {}", info);
    sbi::shutdown()
}
