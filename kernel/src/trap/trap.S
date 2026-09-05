# TrapFrame layout (must match trap/mod.rs):
#   0..256   x0..x31
#   256      sepc
#   264      sstatus
#   272      kernel_sp
#   280..536 f0..f31
#   536      fcsr

.altmacro
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm
.macro SAVE_FP n
    fsd f\n, (280 + \n*8)(a0)
.endm
.macro LOAD_FP n
    fld f\n, (280 + \n*8)(a0)
.endm

    .section .text
    .globl __uservec
    .align 4
__uservec:
    # sscratch holds the TrapFrame pointer; swap with user sp
    csrrw sp, sscratch, sp
    # save general registers (except sp = x2)
    sd x1, 1*8(sp)
    .set n, 3
    .rept 29
        SAVE_GP %n
        .set n, n+1
    .endr
    # user sp
    csrr t0, sscratch
    sd t0, 2*8(sp)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)
    # a0 = trapframe
    mv a0, sp
    # save fp registers
    .set n, 0
    .rept 32
        SAVE_FP %n
        .set n, n+1
    .endr
    frcsr t0
    sd t0, 536(a0)
    # kernel traps now go to __kernelvec
    la t0, __kernelvec
    csrw stvec, t0
    # switch to kernel stack
    ld sp, 272(a0)
    call user_trap_handler
    # user_trap_handler never returns

    # fn __userret(tf: *mut TrapFrame) -> !
    .globl __userret
__userret:
    # a0 = trapframe
    csrw sscratch, a0
    la t0, __uservec
    csrw stvec, t0
    ld t0, 256(a0)
    csrw sepc, t0
    ld t0, 264(a0)
    csrw sstatus, t0
    # restore fp
    .set n, 0
    .rept 32
        LOAD_FP %n
        .set n, n+1
    .endr
    ld t0, 536(a0)
    fscsr t0
    mv sp, a0
    ld x1, 1*8(sp)
    .set n, 3
    .rept 7
        LOAD_GP %n
        .set n, n+1
    .endr
    # skip x10 (a0) for now; load x11..x31
    .set n, 11
    .rept 21
        LOAD_GP %n
        .set n, n+1
    .endr
    ld a0, 10*8(sp)
    ld sp, 2*8(sp)
    sret

    # Kernel-mode trap vector: only interrupts (from the idle loop) and fatal
    # exceptions are expected here. Save everything on the current stack.
    .globl __kernelvec
    .align 4
__kernelvec:
    addi sp, sp, -280
    sd x1, 1*8(sp)
    sd x3, 3*8(sp)
    .set n, 5
    .rept 27
        SAVE_GP %n
        .set n, n+1
    .endr
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)
    mv a0, sp
    call kernel_trap_handler
    ld t0, 264(sp)
    csrw sstatus, t0
    ld t0, 256(sp)
    csrw sepc, t0
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr
    addi sp, sp, 280
    sret

    # fn __switch(cur: *mut Context, next: *const Context)
    # Context: ra, sp, s0..s11, fs0..fs11  (14 + 12 = 26 * 8 = 208 bytes)
    .globl __switch
__switch:
    sd ra, 0(a0)
    sd sp, 8(a0)
    sd s0, 16(a0)
    sd s1, 24(a0)
    sd s2, 32(a0)
    sd s3, 40(a0)
    sd s4, 48(a0)
    sd s5, 56(a0)
    sd s6, 64(a0)
    sd s7, 72(a0)
    sd s8, 80(a0)
    sd s9, 88(a0)
    sd s10, 96(a0)
    sd s11, 104(a0)
    fsd fs0, 112(a0)
    fsd fs1, 120(a0)
    fsd fs2, 128(a0)
    fsd fs3, 136(a0)
    fsd fs4, 144(a0)
    fsd fs5, 152(a0)
    fsd fs6, 160(a0)
    fsd fs7, 168(a0)
    fsd fs8, 176(a0)
    fsd fs9, 184(a0)
    fsd fs10, 192(a0)
    fsd fs11, 200(a0)

    ld ra, 0(a1)
    ld sp, 8(a1)
    ld s0, 16(a1)
    ld s1, 24(a1)
    ld s2, 32(a1)
    ld s3, 40(a1)
    ld s4, 48(a1)
    ld s5, 56(a1)
    ld s6, 64(a1)
    ld s7, 72(a1)
    ld s8, 80(a1)
    ld s9, 88(a1)
    ld s10, 96(a1)
    ld s11, 104(a1)
    fld fs0, 112(a1)
    fld fs1, 120(a1)
    fld fs2, 128(a1)
    fld fs3, 136(a1)
    fld fs4, 144(a1)
    fld fs5, 152(a1)
    fld fs6, 160(a1)
    fld fs7, 168(a1)
    fld fs8, 176(a1)
    fld fs9, 184(a1)
    fld fs10, 192(a1)
    fld fs11, 200(a1)
    ret
