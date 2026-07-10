    .section .text.entry
    .globl _start
_start:
    li      t0, 0
    bne     a0, t0, .park
    la      sp, boot_stack_top
    la      t0, sbss
    la      t1, ebss
.bss_loop:
    beq     t0, t1, .bss_done
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       .bss_loop
.bss_done:
    call    kernel_main

.park:
    wfi
    j .park

    .section .bss.stack
    .align 12
    .globl boot_stack_bottom
boot_stack_bottom:
    .space 524288
    .globl boot_stack_top
boot_stack_top:
