# 循环用户程序：周期性把自己的 pid 数字写到 stdout，用于测试多进程抢占。
    .option norelax
    .section .text
    .global _start
_start:
    # getpid()
    li      a7, 172
    ecall
    # a0 = pid；转成单个数字字符（假设 pid < 10）
    addi    t2, a0, 48      # '0' + pid
    la      t3, buf
    sb      t2, 0(t3)
    li      t2, 10
    sb      t2, 1(t3)       # 换行

    # write(1, buf, 2)
    li      a7, 64
    li      a0, 1
    mv      a1, t3
    li      a2, 2
    ecall

    # 忙等
    li      t0, 0
1:
    addi    t0, t0, 1
    li      t1, 300000
    blt     t0, t1, 1b
    j       _start

    .section .bss
buf:
    .skip 4
