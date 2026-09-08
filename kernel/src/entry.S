    .section .text.entry
    .globl _start
_start:
    # a0 = hartid, a1 = dtb physical address (set by OpenSBI)
    bnez a0, park_hart

    # zero the .bss (everything after the boot stack)
    la t0, sbss
    la t1, ebss
1:  bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    la sp, sbss          # top of BOOT_STACK (the only .bss.stack object)
    call rust_main

park_hart:
3:  wfi
    j 3b
