.section .text.init
.global _start
_start:
    # a0 = hartid, a1 = dtb pointer
    la sp, boot_stack_top
    # clear bss
    la t0, __bss_start
    la t1, __bss_end
1:  bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:  # jump to Rust
    call rust_main
    # never returns
3:  wfi
    j 3b

.section .bss
.align 12
.global boot_stack_bottom
boot_stack_bottom:
    .skip 131072
.global boot_stack_top
boot_stack_top:
