    .section .text
    .globl _start
_start:
    li a0, 1
    la a1, message
    li a2, message_end - message
    li a7, 64
    ecall

    li a0, 0
    li a7, 93
    ecall

1:  j 1b

    .section .rodata
message:
    .ascii "Linux RV64 ELF reached user mode and write(2) works!\n"
message_end:

