// Trap 上下文保存/恢复。S-mode 的 stvec 指向 __alltraps。
// 当前阶段为内核态 trap，直接在当前 sp（内核栈）上保存上下文。
// Phase 5 引入用户态后会改为 sscratch 交换方案。

    .altmacro
    .macro SAVE n
        sd x\n, \n*8(sp)
    .endmacro

    .macro LOAD n
        ld x\n, \n*8(sp)
    .endmacro

    .section .text
    .globl __alltraps
    .globl __restore
    .align 2
__alltraps:
    addi    sp, sp, -288

    // 保存通用寄存器 x1..x31（x0 不必存，但保留槽位便于布局）
    .set i, 1
    .rept 31
        SAVE %i
        .set i, i+1
    .endm
    .rept 0
    .endr

    // 保存 CSR
    csrr    t0, sepc
    sd      t0, 31*8(sp)
    csrr    t0, sstatus
    sd      t0, 32*8(sp)
    csrr    t0, stval
    sd      t0, 33*8(sp)
    csrr    t0, scause
    sd      t0, 34*8(sp)
    csrr    t0, sscratch
    sd      t0, 35*8(sp)

    mv      a0, sp
    call    trap_handler

    mv      sp, a0

__restore:
    ld      t0, 31*8(sp)
    csrw    sepc, t0
    ld      t0, 32*8(sp)
    csrw    sstatus, t0
    ld      t0, 35*8(sp)
    csrw    sscratch, t0

    .set i, 1
    .rept 31
        LOAD %i
        .set i, i+1
    .endr

    addi    sp, sp, 288
    sret
