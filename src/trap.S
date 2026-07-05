// Trap 上下文保存/恢复。S-mode 的 stvec 指向 __alltraps。
// 寄存器布局按 TrapContext 结构体（见 trap.rs）。
//
// x1..x31 共 31 个通用寄存器，加上 sepc/sstatus/stval/scause 等被保存。
// 我们直接把整个上下文放到栈上的 TrapContext 结构里。

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
    csrrw   sp, sscratch, sp      // 交换 sp 与 sscratch
    // 现在 sp 指向内核栈（sscratch 里原来保存的是内核栈指针）
    // 为 TrapContext 在内核栈上预留空间（272 字节足够：31 reg*8 + 5 csrs*8 = 288）
    addi    sp, sp, -288

    // 保存通用寄存器 x1..x31（x0 不必存）
    .set i, 1
    .rept 31
        SAVE %i
        .set i, i+1
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
    // sscratch 里原来的用户栈指针存到这里（暂时为内核栈，无用户态时也无所谓）
    csrr    t0, sscratch
    sd      t0, 35*8(sp)

    // 把 sp 作为参数传给 trap_handler
    mv      a0, sp
    call    trap_handler

    // 返回的 a0 是新的 sp（可能在调度中切换了上下文）
    mv      sp, a0

__restore:
    // 恢复 CSR
    ld      t0, 31*8(sp)
    csrw    sepc, t0
    ld      t0, 32*8(sp)
    csrw    sstatus, t0
    ld      t0, 35*8(sp)
    csrw    sscratch, t0

    // 恢复通用寄存器
    .set i, 1
    .rept 31
        LOAD %i
        .set i, i+1
    .endr

    // 还原 sp：与 sscratch 交换，使其指向用户栈，sscratch 变内核栈
    addi    sp, sp, 288
    csrrw   sp, sscratch, sp

    sret

// 用于从内核态主动进入用户态的入口：a0 = TrapContext 指针
    .globl __restore_from_ctx
__restore_from_ctx:
    mv      sp, a0
    j       __restore
