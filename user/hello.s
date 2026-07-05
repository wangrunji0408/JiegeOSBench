# 最小用户态测试程序：write 一句话后 exit。
# 使用 Linux RISC-V 系统调用 ABI（a7=系统调用号，ecall）。
    .option norelax
    .section .text
    .global _start
_start:
    # write(1, msg, 13)
    li      a7, 64
    li      a0, 1
    la      a1, msg
    li      a2, 13
    ecall
    # exit(0)
    li      a7, 93
    li      a0, 0
    ecall
1:  j       1b

    .section .rodata
msg:
    .ascii "hello user!\n"
