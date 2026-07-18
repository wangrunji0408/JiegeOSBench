    .altmacro
    .macro SAVE_GP n
        sd x\n, \n*8(sp)
    .endm
    .macro LOAD_GP n
        ld x\n, \n*8(sp)
    .endm

    # This code is mapped at the same fixed virtual address (TRAMPOLINE) in
    # every address space, kernel and user alike, so control flow stays
    # valid across the `satp` switch that happens partway through.
    #
    # TrapContext layout (see trap/context.rs), offsets in 8-byte units:
    #   0..32   general registers x0..x31 (x0 unused, always 0)
    #   32      sstatus
    #   33      sepc
    #   34      kernel_satp
    #   35      kernel_sp
    #   36      trap_handler (kernel VA of the Rust trap_handler function)
    .section .text.trampoline
    .globl __alltraps
    .globl __restore
    .align 2
__alltraps:
    # On entry: sp = user stack pointer, sscratch = VA of this task's
    # TrapContext (valid in the currently-active, i.e. user, page table).
    csrrw sp, sscratch, sp
    # now sp -> TrapContext, sscratch -> user's original sp

    sd x1, 1*8(sp)
    sd x3, 3*8(sp)
    sd x4, 4*8(sp)
    .set n, 5
    .rept 27
        SAVE_GP %n
        .set n, n+1
    .endr

    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    csrr t2, sscratch
    sd t2, 2*8(sp)

    ld t0, 34*8(sp)
    ld t1, 36*8(sp)
    ld sp, 35*8(sp)
    csrw satp, t0
    sfence.vma
    jr t1

__restore:
    # a0 = VA of TrapContext (in the target user address space)
    # a1 = user page table token (satp value)
    csrw satp, a1
    sfence.vma
    csrw sscratch, a0
    mv sp, a0

    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1

    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr
    ld x2, 2*8(sp)
    sret
