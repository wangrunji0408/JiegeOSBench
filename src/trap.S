// Trap 上下文保存/恢复。S-mode 的 stvec 指向 __alltraps。
//
// 布局与 TrapContext 结构体（见 trap.rs）严格对应：
//   x[0..32]  -> 0*8 .. 31*8   （x0 保留槽位）
//   sepc      -> 32*8
//   sstatus   -> 33*8
//   stval     -> 34*8
//   scause    -> 35*8
//   sscratch  -> 36*8
// 总共 37 个 8 字节字段 = 296 字节，栈分配对齐到 304。

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
    addi    sp, sp, -304

    // 保存通用寄存器 x1..x31
    .set i, 1
    .rept 31
        SAVE %i
        .set i, i+1
    .endr

    // 保存 CSR
    csrr    t0, sepc
    sd      t0, 32*8(sp)
    csrr    t0, sstatus
    sd      t0, 33*8(sp)
    csrr    t0, stval
    sd      t0, 34*8(sp)
    csrr    t0, scause
    sd      t0, 35*8(sp)
    csrr    t0, sscratch
    sd      t0, 36*8(sp)

    mv      a0, sp
    call    trap_handler

    mv      sp, a0

__restore:
    ld      t0, 32*8(sp)
    csrw    sepc, t0
    ld      t0, 33*8(sp)
    csrw    sstatus, t0
    ld      t0, 36*8(sp)
    csrw    sscratch, t0

    .set i, 1
    .rept 31
        LOAD %i
        .set i, i+1
    .endr

    addi    sp, sp, 304
    sret
