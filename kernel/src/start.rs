//! 内核入口汇编

global_asm!(
    r#"
.section .text.init
.globl _start
_start:
    # OpenSBI 进入: a0 = hartid, a1 = dtb 物理地址
    # 关闭中断
    csrw sie, zero

    # 设置全局指针
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    # 清理 BSS
    la t0, __bss_start
    la t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    # 引导栈（位于 .bss 之后的静态区，由链接脚本前的符号定位）
    la sp, _boot_stack_top
    call rust_main
    # 不应返回
3:  wfi
    j 3b
"#
);
