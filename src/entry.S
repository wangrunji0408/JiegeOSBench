// 内核入口汇编。OpenSBI 在 M-mode 把控制权交给 S-mode 的 _start，
// 此时 a0 = hartid, a1 = dtb 物理地址。我们在单核下只用 hart 0。
    .section .text.init
    .globl _start
_start:
    // 关闭所有中断，先清 sstatus.SIE / SPIE
    csrw    sie, zero
    // 只允许 hart 0 继续运行，其它 hart 挂起
    bnez    a0, park

    // 设立栈：在内核镜像后方预留 64KiB 栈
    la      sp, _stack_top

    // 清零 .bss
    la      t0, __bss_start
    la      t1, __bss_end
1:
    bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b
2:

    // 跳转到 Rust 入口
    call    rust_main

park:
    wfi
    j       park

// 64KiB 栈空间
    .section .bss
    .align 12
_stack_bottom:
    .skip 65536
_stack_top:
