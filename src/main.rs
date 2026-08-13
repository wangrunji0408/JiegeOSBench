#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]
#![allow(dead_code)]

mod console;
mod lang;
mod sbi;
mod sync;
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

    println!("[iJiege kernel] kernel initialized. entering idle loop...");
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}
