#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]
#![allow(dead_code)]
#![feature(alloc_error_handler)]

#[macro_use]
extern crate alloc;

mod console;
mod fs;
mod lang;
mod memory;
mod process;
mod sbi;
mod sync;
mod syscall;
mod trap;

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    # zero .bss
    la t0, _bss_start
    la t1, _bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    # park secondary harts
    bnez a0, 3f
    call rust_main
3:
    wfi
    j 3b

    .section .bss
    .align 12
boot_stack:
    .space 64 * 1024
boot_stack_top:
"#
);

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, _dtb: usize) -> ! {
    console::init();
    println!("\n[iJiege kernel] booting on hart {hartid}");
    println!("[iJiege kernel] RISC-V Sv39, QEMU virt machine");

    trap::init();
    sbi::init_timer();
    memory::init();

    // smoke test the allocator
    let mut v = alloc::vec::Vec::new();
    v.push(42usize);
    v.push(1337usize);
    println!("[mem] allocator smoke test: {:?}", v);
    core::mem::drop(v);

    // Spawn the first user process (embedded static ELF).
    let hello = include_bytes!("../user/hello");
    let proc = process::Process::from_elf(hello, &["hello", "world"], &[]);
    let cx = proc.trap_cx_ptr();
    proc.activate();
    *process::current().lock() = Some(proc);
    unsafe {
        println!("[kernel] entering user mode (entry={:#x}, sp={:#x})", (*cx).sepc, (*cx).x[2]);
    }
    trap::switch_to(cx);
}

static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Called from the trap handler on each supervisor timer interrupt.
pub fn timer_tick() {
    let now = sbi::get_time();
    // 10ms tick
    sbi::set_timer(now + 10_000_000);
    let n = TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    if n % 100 == 0 {
        println!("[tick] {} ({} ms)", n, n * 10);
    }
}
