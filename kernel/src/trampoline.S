    .altmacro
    .macro SAVE_GP n
        sd x\n, \n*8(a0)
    .endm
    .macro LOAD_GP n
        ld x\n, \n*8(a0)
    .endm

    # This code is mapped at the same fixed virtual address (TRAMPOLINE) in
    # every address space, kernel and user alike, so it stays valid across
    # the `satp` switch that happens partway through.
    .section .text.trampoline
    .globl __alltraps
    .globl __restore
    .align 2
__alltraps:
    # On entry: sp = user stack pointer, sscratch = VA of this task's
    # TrapContext (in the CURRENTLY ACTIVE, i.e. user, page table).
    csrrw sp, sscratch, sp
    # now sp -> TrapContext, sscratch -> user sp

    sd x1, 1*8(sp)
    # x2 (sp) saved below via sscratch
    sd x3, 3*8(sp)
    sd x4, 4*8(sp)
    .set n, 5
    .rept 27
        SAVE_GP %n, sp
        .set n, n+1
    .endr
