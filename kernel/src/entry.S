    .section .text.entry
    .globl _start
_start:
    # a0 = hartid, a1 = dtb
    la sp, __boot_stack_top
    # clear bss
    la t0, __bss_start
    la t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    call rust_main
3:
    wfi
    j 3b

    .section .bss.stack
    .globl __boot_stack_bottom
__boot_stack_bottom:
    .space 1024 * 128
    .globl __boot_stack_top
__boot_stack_top:
