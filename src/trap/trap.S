// Trap entry / exit and the task context switch.
//
// TrapContext layout (see context.rs), byte offsets:
//   0..256   x[0..32]
//   256      sepc
//   264      sstatus
//   272      kernel_sp
//   280..536 f[0..32]
//   536      fcsr
//   size     544
//
// While running in user mode, `sscratch` holds the address of the current
// task's TrapContext, which sits at the top of its kernel stack. While running
// in the kernel, `sscratch` is zero, which is how we distinguish a trap taken
// from user mode from one taken in the kernel.

.equ CTX_SEPC,      256
.equ CTX_SSTATUS,   264
.equ CTX_KERNEL_SP, 272
.equ CTX_F,         280
.equ CTX_FCSR,      536
.equ CTX_SIZE,      544

.equ SSTATUS_SPP,      0x100
.equ SSTATUS_FS_DIRTY, 0x6000

.altmacro
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm
.macro SAVE_FP n, base
    fsd f\n, \n*8+CTX_F(\base)
.endm
.macro LOAD_FP n, base
    fld f\n, \n*8+CTX_F(\base)
.endm

    .section .text
    .globl __trap_entry
    .align 4
__trap_entry:
    // Distinguish user vs kernel trap without clobbering anything: swap sp with
    // sscratch. If the result is zero we came from the kernel (sscratch == 0),
    // so swap back and take the kernel path.
    csrrw sp, sscratch, sp
    bnez sp, __user_trap
    csrrw sp, sscratch, sp
    j __kernel_trap

// ---------------------------------------------------------------------------
// Trap from user mode. sp = &TrapContext, sscratch = user sp.
// ---------------------------------------------------------------------------
__user_trap:
    // Save general purpose registers. x0 is constant, and x2 (sp) currently
    // holds the context pointer, so handle it separately below.
    sd x1, 1*8(sp)
    .set n, 3
    .rept 29
        SAVE_GP %n
        .set n, n+1
    .endr

    // Recover the user sp from sscratch and store it, then clear sscratch to
    // mark that we are now in the kernel.
    csrrw t0, sscratch, zero
    sd t0, 2*8(sp)

    csrr t1, sepc
    csrr t2, sstatus
    sd t1, CTX_SEPC(sp)
    sd t2, CTX_SSTATUS(sp)

    // Save the FPU only if the user dirtied it since the last restore.
    // SSTATUS_FS_DIRTY is 0x6000, outside andi's 12-bit immediate range, so the
    // mask goes through a register.
    li   t4, SSTATUS_FS_DIRTY
    and  t3, t2, t4
    bne  t3, t4, 1f
    mv   t5, sp
    .set n, 0
    .rept 32
        SAVE_FP %n, t5
        .set n, n+1
    .endr
    frcsr t6
    sd t6, CTX_FCSR(t5)
1:
    // Hand the context to Rust and switch onto the kernel stack, which grows
    // down from just below the context.
    mv a0, sp
    ld sp, CTX_KERNEL_SP(sp)
    call trap_from_user

// Fall through: return to user space with the context in a0.
    .globl __trap_return
__trap_return:
    // a0 = &TrapContext
    mv sp, a0
    // Re-arm sscratch so the next user trap finds the context.
    csrw sscratch, sp

    ld t0, CTX_SEPC(sp)
    ld t1, CTX_SSTATUS(sp)
    csrw sepc, t0
    csrw sstatus, t1

    // Restore the FPU if the context says it is live.
    li   t2, SSTATUS_FS_DIRTY
    and  t2, t1, t2
    beqz t2, 2f
    mv   t3, sp
    ld   t4, CTX_FCSR(t3)
    fscsr t4
    .set n, 0
    .rept 32
        LOAD_FP %n, t3
        .set n, n+1
    .endr
2:
    ld x1, 1*8(sp)
    .set n, 3
    .rept 29
        LOAD_GP %n
        .set n, n+1
    .endr
    // Finally the user sp.
    ld sp, 2*8(sp)
    sret

// ---------------------------------------------------------------------------
// Trap taken while running in the kernel (interrupt, or a kernel fault).
// We stay on the current stack and save a minimal frame.
// ---------------------------------------------------------------------------
    .align 4
__kernel_trap:
    addi sp, sp, -34*8
    sd x1,  1*8(sp)
    .set n, 3
    .rept 29
        SAVE_GP %n
        .set n, n+1
    .endr
    csrr t0, sepc
    csrr t1, sstatus
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)

    mv a0, sp
    call trap_from_kernel

    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sepc, t0
    csrw sstatus, t1
    ld x1, 1*8(sp)
    .set n, 3
    .rept 29
        LOAD_GP %n
        .set n, n+1
    .endr
    addi sp, sp, 34*8
    sret

// ---------------------------------------------------------------------------
// void __switch(TaskContext *current, const TaskContext *next)
// Save callee-saved state into *current and resume *next.
// ---------------------------------------------------------------------------
    .globl __switch
__switch:
    sd ra, 0(a0)
    sd sp, 8(a0)
    sd s0,  16(a0)
    sd s1,  24(a0)
    sd s2,  32(a0)
    sd s3,  40(a0)
    sd s4,  48(a0)
    sd s5,  56(a0)
    sd s6,  64(a0)
    sd s7,  72(a0)
    sd s8,  80(a0)
    sd s9,  88(a0)
    sd s10, 96(a0)
    sd s11, 104(a0)

    ld ra, 0(a1)
    ld sp, 8(a1)
    ld s0,  16(a1)
    ld s1,  24(a1)
    ld s2,  32(a1)
    ld s3,  40(a1)
    ld s4,  48(a1)
    ld s5,  56(a1)
    ld s6,  64(a1)
    ld s7,  72(a1)
    ld s8,  80(a1)
    ld s9,  88(a1)
    ld s10, 96(a1)
    ld s11, 104(a1)
    ret
