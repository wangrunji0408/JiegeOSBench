// Trap 上下文保存/恢复，支持 S-mode 与 U-mode 双来源。
//
// TrapContext 布局（与 trap.rs 一致）：
//   x[0..32] -> 0*8 .. 31*8     （x[2]=sp 单独处理）
//   sepc     -> 32*8
//   sstatus  -> 33*8
//   stval    -> 34*8
//   scause   -> 35*8
// 共 36 字段 = 288 字节，栈分配对齐到 304。
//
// S-mode 进入时 sscratch=0；U-mode 进入时 sscratch=内核栈指针。
// 通过 sstatus.SPP 区分来源。

#define CTX_SIZE 304

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
    # 检查来源：sstatus.SPP (bit 8)
    csrr    t0, sstatus
    andi    t0, t0, (1 << 8)
    bnez    t0, .Lfrom_s

    # U-mode 来源：sp 与 sscratch 交换，得到内核栈
    csrrw   sp, sscratch, sp
.Lfrom_s:
    addi    sp, sp, -CTX_SIZE

    # 保存 x1, x3..x31
    .set i, 1
    .rept 31
        .ifne (i - 2)
            SAVE %i
        .endif
        .set i, i+1
    .endr

    # 保存 x2(sp)：U-mode 取 sscratch（用户 sp），S-mode 取 sp+CTX_SIZE（内核 sp）
    csrr    t0, sstatus
    andi    t0, t0, (1 << 8)
    bnez    t0, .Ls_sp
    csrr    t0, sscratch            # U-mode: 用户 sp
    sd      t0, 2*8(sp)
    j       .Lsave_csr
.Ls_sp:
    addi    t0, sp, CTX_SIZE        # S-mode: 保存前的内核 sp
    sd      t0, 2*8(sp)

.Lsave_csr:
    csrr    t0, sepc
    sd      t0, 32*8(sp)
    csrr    t0, sstatus
    sd      t0, 33*8(sp)
    csrr    t0, stval
    sd      t0, 34*8(sp)
    csrr    t0, scause
    sd      t0, 35*8(sp)

    mv      a0, sp
    call    trap_handler

    mv      sp, a0

__restore:
    # 恢复 CSR
    ld      t0, 32*8(sp)
    csrw    sepc, t0
    ld      t0, 33*8(sp)
    csrw    sstatus, t0

    # 恢复 x1, x3..x31
    .set i, 1
    .rept 31
        .ifne (i - 2)
            LOAD %i
        .endif
        .set i, i+1
    .endr

    # 恢复 x2 与 sscratch，分 U/S 两种返回
    csrr    t0, sstatus
    andi    t0, t0, (1 << 8)
    bnez    t0, .Lret_s

    # 返回 U-mode：sscratch = 内核栈顶(sp+CTX_SIZE)，sp = 用户 sp
    ld      t0, 2*8(sp)             # 用户 sp
    addi    t1, sp, CTX_SIZE        # 内核栈顶
    csrw    sscratch, t1
    mv      sp, t0
    sret

.Lret_s:
    # 返回 S-mode：sp = 保存的内核 sp
    ld      t0, 2*8(sp)
    mv      sp, t0
    sret
