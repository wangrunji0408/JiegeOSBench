    .attribute arch, "rv64gc"
    .section .text
    .globl __alltraps
    .globl __restore
    .globl __switch

// sscratch约定:
//   用户态时: sscratch = 内核栈顶
//   内核态时: sscratch = 0（通过在.from_user分支立即写0来保证）

__alltraps:
    // 先交换sp和sscratch
    csrrw sp, sscratch, sp
    // 现在：sp=旧sscratch，sscratch=旧sp
    // 如果旧sscratch=0（内核态），新sp=0，这样bnez判断失败
    // 如果旧sscratch=内核栈顶（用户态），新sp=内核栈顶

    beqz sp, .from_kernel
    // 走.from_user路径

.from_user:
    // sp = 内核栈顶
    // sscratch = 用户sp
    // 分配TrapContext空间
    addi sp, sp, -37*8
    // 保存通用寄存器（除了x2=sp）
    sd ra,  1*8(sp)
    sd gp,  3*8(sp)
    sd tp,  4*8(sp)
    sd t0,  5*8(sp)
    sd t1,  6*8(sp)
    sd t2,  7*8(sp)
    sd s0,  8*8(sp)
    sd s1,  9*8(sp)
    sd a0,  10*8(sp)
    sd a1,  11*8(sp)
    sd a2,  12*8(sp)
    sd a3,  13*8(sp)
    sd a4,  14*8(sp)
    sd a5,  15*8(sp)
    sd a6,  16*8(sp)
    sd a7,  17*8(sp)
    sd s2,  18*8(sp)
    sd s3,  19*8(sp)
    sd s4,  20*8(sp)
    sd s5,  21*8(sp)
    sd s6,  22*8(sp)
    sd s7,  23*8(sp)
    sd s8,  24*8(sp)
    sd s9,  25*8(sp)
    sd s10, 26*8(sp)
    sd s11, 27*8(sp)
    sd t3,  28*8(sp)
    sd t4,  29*8(sp)
    sd t5,  30*8(sp)
    sd t6,  31*8(sp)
    // 保存用户sp（从sscratch读取）
    csrr t0, sscratch    // t0 = 用户sp
    sd t0, 2*8(sp)       // 保存用户sp
    // 立即把sscratch清0（表示现在在内核态）
    csrw sscratch, zero
    // 保存CSR
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    // 保存用户页表satp到_reserved[0] (slot 34)
    csrr t0, satp
    sd t0, 34*8(sp)
    // 切换到内核页表 (KERNEL_SATP)
    la t1, KERNEL_SATP
    ld t0, 0(t1)
    beqz t0, .skip_pt_switch_in   // 如果KERNEL_SATP=0，跳过切换
    csrr t1, satp
    beq t0, t1, .skip_pt_switch_in  // 如果已经是内核PT，跳过
    csrw satp, t0
    sfence.vma
.skip_pt_switch_in:
    mv a0, sp
    call trap_handler
    mv sp, a0
    j .restore_user

.from_kernel:
    // sp = 0（从sscratch读到的0），sscratch = 内核sp
    // 把sscratch恢复到sp
    csrrw sp, sscratch, sp
    // 现在 sp = 内核sp，sscratch = 0
    // 在内核栈上分配空间处理中断
    addi sp, sp, -37*8
    sd ra,  1*8(sp)
    sd gp,  3*8(sp)
    sd tp,  4*8(sp)
    sd t0,  5*8(sp)
    sd t1,  6*8(sp)
    sd t2,  7*8(sp)
    sd s0,  8*8(sp)
    sd s1,  9*8(sp)
    sd a0,  10*8(sp)
    sd a1,  11*8(sp)
    sd a2,  12*8(sp)
    sd a3,  13*8(sp)
    sd a4,  14*8(sp)
    sd a5,  15*8(sp)
    sd a6,  16*8(sp)
    sd a7,  17*8(sp)
    sd s2,  18*8(sp)
    sd s3,  19*8(sp)
    sd s4,  20*8(sp)
    sd s5,  21*8(sp)
    sd s6,  22*8(sp)
    sd s7,  23*8(sp)
    sd s8,  24*8(sp)
    sd s9,  25*8(sp)
    sd s10, 26*8(sp)
    sd s11, 27*8(sp)
    sd t3,  28*8(sp)
    sd t4,  29*8(sp)
    sd t5,  30*8(sp)
    sd t6,  31*8(sp)
    // 保存内核sp（sp+37*8是中断前的sp）
    addi t0, sp, 37*8
    sd t0, 2*8(sp)
    // 保存CSR
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    mv a0, sp
    call trap_handler
    // 从内核trap返回
    mv sp, a0
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1
    ld ra,  1*8(sp)
    ld gp,  3*8(sp)
    ld tp,  4*8(sp)
    ld t0,  5*8(sp)
    ld t1,  6*8(sp)
    ld t2,  7*8(sp)
    ld s0,  8*8(sp)
    ld s1,  9*8(sp)
    ld a0,  10*8(sp)
    ld a1,  11*8(sp)
    ld a2,  12*8(sp)
    ld a3,  13*8(sp)
    ld a4,  14*8(sp)
    ld a5,  15*8(sp)
    ld a6,  16*8(sp)
    ld a7,  17*8(sp)
    ld s2,  18*8(sp)
    ld s3,  19*8(sp)
    ld s4,  20*8(sp)
    ld s5,  21*8(sp)
    ld s6,  22*8(sp)
    ld s7,  23*8(sp)
    ld s8,  24*8(sp)
    ld s9,  25*8(sp)
    ld s10, 26*8(sp)
    ld s11, 27*8(sp)
    ld t3,  28*8(sp)
    ld t4,  29*8(sp)
    ld t5,  30*8(sp)
    ld t6,  31*8(sp)
    ld sp, 2*8(sp)   // 恢复内核sp
    sret

.restore_user:
    // sp = TrapContext地址（在内核栈上）
    // sscratch = 0
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1
    // 计算内核栈顶：TrapContext + 37*8
    addi t0, sp, 37*8  // t0 = 内核栈顶
    csrw sscratch, t0  // sscratch = 内核栈顶（为下次trap准备）
    // 切换回用户页表（从_reserved[0] = slot 34 中恢复）
    ld t0, 34*8(sp)    // t0 = 用户satp (saved earlier)
    beqz t0, .skip_pt_restore  // 如果为0，跳过
    csrr t1, satp
    beq t0, t1, .skip_pt_restore  // 如果已经是用户PT，跳过
    csrw satp, t0
    sfence.vma
.skip_pt_restore:
    // 恢复通用寄存器
    ld ra,  1*8(sp)
    // 暂时跳过x2（用户sp），需要最后恢复
    ld gp,  3*8(sp)
    ld tp,  4*8(sp)
    ld t0,  5*8(sp)
    ld t1,  6*8(sp)
    ld t2,  7*8(sp)
    ld s0,  8*8(sp)
    ld s1,  9*8(sp)
    ld a0,  10*8(sp)
    ld a1,  11*8(sp)
    ld a2,  12*8(sp)
    ld a3,  13*8(sp)
    ld a4,  14*8(sp)
    ld a5,  15*8(sp)
    ld a6,  16*8(sp)
    ld a7,  17*8(sp)
    ld s2,  18*8(sp)
    ld s3,  19*8(sp)
    ld s4,  20*8(sp)
    ld s5,  21*8(sp)
    ld s6,  22*8(sp)
    ld s7,  23*8(sp)
    ld s8,  24*8(sp)
    ld s9,  25*8(sp)
    ld s10, 26*8(sp)
    ld s11, 27*8(sp)
    ld t3,  28*8(sp)
    ld t4,  29*8(sp)
    ld t5,  30*8(sp)
    ld t6,  31*8(sp)
    // 最后恢复sp（用户sp）
    ld sp, 2*8(sp)
    sret

__restore:
    // sp = TrapContext地址（内核栈上）
    j .restore_user

__switch:
    sd ra,  0*8(a0)
    sd sp,  1*8(a0)
    sd s0,  2*8(a0)
    sd s1,  3*8(a0)
    sd s2,  4*8(a0)
    sd s3,  5*8(a0)
    sd s4,  6*8(a0)
    sd s5,  7*8(a0)
    sd s6,  8*8(a0)
    sd s7,  9*8(a0)
    sd s8,  10*8(a0)
    sd s9,  11*8(a0)
    sd s10, 12*8(a0)
    sd s11, 13*8(a0)
    ld ra,  0*8(a1)
    ld sp,  1*8(a1)
    ld s0,  2*8(a1)
    ld s1,  3*8(a1)
    ld s2,  4*8(a1)
    ld s3,  5*8(a1)
    ld s4,  6*8(a1)
    ld s5,  7*8(a1)
    ld s6,  8*8(a1)
    ld s7,  9*8(a1)
    ld s8,  10*8(a1)
    ld s9,  11*8(a1)
    ld s10, 12*8(a1)
    ld s11, 13*8(a1)
    ret
