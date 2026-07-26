//! Kernel entry point: set up a boot stack and jump into Rust.

use core::arch::global_asm;

/// Boot stack size per hart (512 KiB). The kernel runs deep recursive code in
/// the ELF loader and network stack, so keep this generous.
pub const BOOT_STACK_SIZE: usize = 512 * 1024;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    # a0 = hartid, a1 = device tree blob (from OpenSBI)
    # Only hart 0 boots; park the others.
    bnez a0, .Lpark

    la sp, boot_stack_top
    mv tp, a0
    j rust_main

.Lpark:
    wfi
    j .Lpark

    .section .bss.stack
    .globl boot_stack_bottom
boot_stack_bottom:
    .space {stack_size}
    .globl boot_stack_top
boot_stack_top:
"#,
    stack_size = const BOOT_STACK_SIZE,
);
