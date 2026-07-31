.section .text
.global trap_entry
.align 2
trap_entry:
    # sscratch = current task kernel stack top (invariant, set by scheduler)
    # swap tp with sscratch: tp = kernel stack top, sscratch = old tp (preserved)
    csrrw tp, sscratch, tp
    # save the interrupted sp into tf slot regs[2]
    #   (U-mode: user sp
 S-mode: kernel sp at entry)
    sd sp, -536+16(tp)
    # switch to the kernel stack (sp = trapframe base)
    addi sp, tp, -536
    sd x1, 8(sp)          # ra
    sd x5, 40(sp)         # user t0 (before we clobber it)
    csrr t0, sscratch     # t0 = old tp (user's tp for U traps, kernel tp for S traps)
    sd t0, 32(sp)         # regs[4] = tp
    csrw sscratch, tp     # restore invariant: sscratch = kernel stack top
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
    sd x31, 248(sp)       # user t6 (before we clobber it)
    csrr t6, sstatus
    srli t6, t6, 8
    andi t6, t6, 1        # t6 = SPP (1 = from S-mode)
    # (no further clobbering of user registers)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)

    # save FP registers (f0-f31 at 272..528, fcsr at 528)
    fsd f0, 272(sp)
    fsd f1, 280(sp)
    fsd f2, 288(sp)
    fsd f3, 296(sp)
    fsd f4, 304(sp)
    fsd f5, 312(sp)
    fsd f6, 320(sp)
    fsd f7, 328(sp)
    fsd f8, 336(sp)
    fsd f9, 344(sp)
    fsd f10, 352(sp)
    fsd f11, 360(sp)
    fsd f12, 368(sp)
    fsd f13, 376(sp)
    fsd f14, 384(sp)
    fsd f15, 392(sp)
    fsd f16, 400(sp)
    fsd f17, 408(sp)
    fsd f18, 416(sp)
    fsd f19, 424(sp)
    fsd f20, 432(sp)
    fsd f21, 440(sp)
    fsd f22, 448(sp)
    fsd f23, 456(sp)
    fsd f24, 464(sp)
    fsd f25, 472(sp)
    fsd f26, 480(sp)
    fsd f27, 488(sp)
    fsd f28, 496(sp)
    fsd f29, 504(sp)
    fsd f30, 512(sp)
    fsd f31, 520(sp)
    csrr t0, fcsr
    sw t0, 528(sp)

    mv a0, sp          # &mut TrapFrame
    csrr a1, scause
    csrr a2, sepc
    csrr a3, stval
    call trap_handler
    mv sp, a0          # possibly different task's trapframe

trap_restore:
    ld t0, 264(sp)
    csrw sstatus, t0
    ld t0, 256(sp)
    csrw sepc, t0
    csrw sscratch, tp     # sscratch = kernel stack top (tp still holds it)
    ld x1, 8(sp)
    ld x3, 24(sp)
    ld x4, 32(sp)         # old tp
    ld x5, 40(sp)         # old t0
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

    # restore FP registers
    fld f0, 272(sp)
    fld f1, 280(sp)
    fld f2, 288(sp)
    fld f3, 296(sp)
    fld f4, 304(sp)
    fld f5, 312(sp)
    fld f6, 320(sp)
    fld f7, 328(sp)
    fld f8, 336(sp)
    fld f9, 344(sp)
    fld f10, 352(sp)
    fld f11, 360(sp)
    fld f12, 368(sp)
    fld f13, 376(sp)
    fld f14, 384(sp)
    fld f15, 392(sp)
    fld f16, 400(sp)
    fld f17, 408(sp)
    fld f18, 416(sp)
    fld f19, 424(sp)
    fld f20, 432(sp)
    fld f21, 440(sp)
    fld f22, 448(sp)
    fld f23, 456(sp)
    fld f24, 464(sp)
    fld f25, 472(sp)
    fld f26, 480(sp)
    fld f27, 488(sp)
    fld f28, 496(sp)
    fld f29, 504(sp)
    fld f30, 512(sp)
    fld f31, 520(sp)
    lw t0, 528(sp)
    csrw fcsr, t0

    ld sp, 16(sp)         # user sp (U) or kernel sp (S) - stored at entry
    sret

# Enter user mode with a prepared trapframe (used at boot for the first task).
.global enter_user
enter_user:
    mv sp, a0
    j trap_restore

# First-run stub: after switch_to's epilogue, sp points at the task's trapframe.
.global first_run_stub
first_run_stub:
    j trap_restore

# context switch: switch_to(&mut current_ctx: *mut usize, &next_ctx: *mut usize)
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
