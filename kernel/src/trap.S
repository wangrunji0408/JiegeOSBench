.section .text
.global trap_entry
.align 2
trap_entry:
    # save sstatus to check SPP (0 = from U-mode, 1 = from S-mode)
    csrr t6, sstatus
    srli t6, t6, 8
    andi t6, t6, 1
    bnez t6, 1f
    # from U-mode: swap sp with sscratch (sscratch = current task kernel stack top)
    csrrw sp, sscratch, sp
1:
    addi sp, sp, -272
    sd x1, 8(sp)      # ra
    sd x2, 16(sp)     # sp
    sd x3, 24(sp)     # gp
    sd x4, 32(sp)     # tp
    sd x5, 40(sp)
    sd x6, 48(sp)
    sd x7, 56(sp)
    sd x8, 64(sp)
    sd x9, 72(sp)
    sd x10, 80(sp)
    sd x11, 88(sp)
    sd x12, 96(sp)
    sd x13, 104(sp)
    sd x14, 112(sp)
    sd x15, 120(sp)
    sd x16, 128(sp)
    sd x17, 136(sp)
    sd x18, 144(sp)
    sd x19, 152(sp)
    sd x20, 160(sp)
    sd x21, 168(sp)
    sd x22, 176(sp)
    sd x23, 184(sp)
    sd x24, 192(sp)
    sd x25, 200(sp)
    sd x26, 208(sp)
    sd x27, 216(sp)
    sd x28, 224(sp)
    sd x29, 232(sp)
    sd x30, 240(sp)
    sd x31, 248(sp)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)

    mv a0, sp          # &mut TrapFrame
    csrr a1, scause
    csrr a2, sepc
    csrr a3, stval
    call trap_handler
    mv sp, a0          # possibly different task's trapframe

    ld t0, 256(sp)
    csrw sepc, t0
    ld t0, 264(sp)
    csrw sstatus, t0
    ld x1, 8(sp)
    # sp restored below (skip x2 here)
    ld x3, 24(sp)
    ld x4, 32(sp)
    ld x5, 40(sp)
    ld x6, 48(sp)
    ld x7, 56(sp)
    ld x8, 64(sp)
    ld x9, 72(sp)
    ld x10, 80(sp)
    ld x11, 88(sp)
    ld x12, 96(sp)
    ld x13, 104(sp)
    ld x14, 112(sp)
    ld x15, 120(sp)
    ld x16, 128(sp)
    ld x17, 136(sp)
    ld x18, 144(sp)
    ld x19, 152(sp)
    ld x20, 160(sp)
    ld x21, 168(sp)
    ld x22, 176(sp)
    ld x23, 184(sp)
    ld x24, 192(sp)
    ld x25, 200(sp)
    ld x26, 208(sp)
    ld x27, 216(sp)
    ld x28, 224(sp)
    ld x29, 232(sp)
    ld x30, 240(sp)
    ld x31, 248(sp)
    addi sp, sp, 272   # sp = kernel stack top (for U traps) or original kernel sp

    csrr t6, sstatus
    srli t6, t6, 8
    andi t6, t6, 1
    bnez t6, 2f
    # restore user sp
    csrrw sp, sscratch, sp
2:
    sret

# Enter user mode with a prepared trapframe (sp = tf address after pop).
# Used at boot to start the first task.
.global enter_user
enter_user:
    mv sp, a0
    j trap_restore

# First-run stub: after switch_to's epilogue, sp points at the task's trapframe.
.global first_run_stub
first_run_stub:
    j trap_restore

# context switch: switch_to(&mut current_ctx: *mut usize, &next_ctx: *mut usize)
# saves callee-saved regs of current task on its kernel stack, restores next's.
.global switch_to
.align 2
switch_to:
    addi sp, sp, -13*8
    sd ra, 0*8(sp)
    sd s0, 1*8(sp)
    sd s1, 2*8(sp)
    sd s2, 3*8(sp)
    sd s3, 4*8(sp)
    sd s4, 5*8(sp)
    sd s5, 6*8(sp)
    sd s6, 7*8(sp)
    sd s7, 8*8(sp)
    sd s8, 9*8(sp)
    sd s9, 10*8(sp)
    sd s10, 11*8(sp)
    sd s11, 12*8(sp)
    sd sp, 0(a0)       # *current_ctx = sp (saved sp)
    ld sp, 0(a1)       # sp = *next_ctx
    ld ra, 0*8(sp)
    ld s0, 1*8(sp)
    ld s1, 2*8(sp)
    ld s2, 3*8(sp)
    ld s3, 4*8(sp)
    ld s4, 5*8(sp)
    ld s5, 6*8(sp)
    ld s6, 7*8(sp)
    ld s7, 8*8(sp)
    ld s8, 9*8(sp)
    ld s9, 10*8(sp)
    ld s10, 11*8(sp)
    ld s11, 12*8(sp)
    addi sp, sp, 13*8
    ret
