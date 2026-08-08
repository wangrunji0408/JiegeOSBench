.section .text.boot
.globl _start
.type _start, @function
_start:
    li t0, 0x10000000
    li t1, 0x4b
    sb t1, 0(t0)
    la sp, boot_stack_top
    call rust_entry
1:
    wfi
    j 1b

.section .bss.stack
.align 12
boot_stack:
    .space 16384
boot_stack_top:
