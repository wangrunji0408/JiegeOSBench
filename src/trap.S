// Trap 上下文保存/恢复，支持 S-mode 与 U-mode 双来源。
//
// TrapContext 布局（与 trap.rs 一致）：
//   x[0..32] -> 0*8 .. 31*8     （x[2]=sp 在 restore 末尾用 mem->sp 直接恢复）
//   sepc     -> 32*8
//   sstatus  -> 33*8
//   stval    -> 34*8
//   scause   -> 35*8
// 共 36 字段 = 288 字节，栈分配对齐到 304。
//
// S-mode 进入时 sscratch=0；U-mode 进入时 sscratch=内核栈顶。
// 通过 sstatus.SPP 区分来源。restore 末尾 sscratch 统一设为内核栈顶。

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

    # U-mode：sp 与 sscratch 交换，得到内核栈
    csrrw   sp, sscratch, sp
.Lfrom_s:
    addi    sp, sp, -CTX_SIZE

    # 保存 x1, x3..x31（跳过 x2）
    .set i, 1
    .rept 31
        .ifne (i - 2)
            SAVE %i
        .endif
        .set i, i+1
    .endr

    # 保存 x2：U-mode 取 sscratch(用户 sp)，S-mode 取 sp+CTX_SIZE(内核 sp)
    csrr    t0, sstatus
    andi    t0, t0, (1 << 8)
    bnez    t0, .Ls_sp
    csrr    t0, sscratch
    sd      t0, 2*8(sp)
    j       .Lsave_csr
.Ls_sp:
    addi    t0, sp, CTX_SIZE
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
    # 恢复 CSR（用 t0 作 scratch）
    ld      t0, 33*8(sp)
    csrw    sstatus, t0
    ld      t0, 32*8(sp)
    csrw    sepc, t0

    # 先设 sscratch = 内核栈顶（sp + CTX_SIZE），供下次 U-mode trap 使用
    addi    t0, sp, CTX_SIZE
    csrw    sscratch, t0

    # 恢复 x1, x3..x31（跳过 x2，最后单独恢复 sp）
    .set i, 1
    .rept 31
        .ifne (i - 2)
            LOAD %i
        .endif
        .set i, i+1
    .endr

    # 最后恢复 sp：从 [2*8(sp)] 直接装入 sp（旧 sp 作基址读取后丢弃）
    ld      sp, 2*8(sp)
    sret
