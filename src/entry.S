.section .text.entry
.globl _start
_start:
    bnez a0, 2f
    la sp, boot_stack_top
    la t0, sbss
    la t1, ebss
1:  bgeu t0, t1, 3f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
3:  la sp, boot_stack_top
    call kmain
2:  wfi
    j 2b

.section .bss
.align 12
boot_stack:
    .space 262144
.globl boot_stack_top
boot_stack_top:

.section .text
.align 4
.globl user_trap
user_trap:
    csrrw sp, sscratch, sp
    addi sp, sp, -272
    sd ra, 8(sp)
    sd gp, 24(sp)
    sd tp, 32(sp)
    sd t0, 40(sp)
    sd t1, 48(sp)
    sd t2, 56(sp)
    sd s0, 64(sp)
    sd s1, 72(sp)
    sd a0, 80(sp)
    sd a1, 88(sp)
    sd a2, 96(sp)
    sd a3, 104(sp)
    sd a4, 112(sp)
    sd a5, 120(sp)
    sd a6, 128(sp)
    sd a7, 136(sp)
    sd s2, 144(sp)
    sd s3, 152(sp)
    sd s4, 160(sp)
    sd s5, 168(sp)
    sd s6, 176(sp)
    sd s7, 184(sp)
    sd s8, 192(sp)
    sd s9, 200(sp)
    sd s10, 208(sp)
    sd s11, 216(sp)
    sd t3, 224(sp)
    sd t4, 232(sp)
    sd t5, 240(sp)
    sd t6, 248(sp)
    csrr t0, sscratch
    sd t0, 16(sp)
    csrr t0, sepc
    sd t0, 256(sp)
    csrr t0, sstatus
    sd t0, 264(sp)
    la t0, kernel_trap
    csrw stvec, t0
    mv a0, sp
    call rust_trap
    mv a0, sp
    j enter_user

.globl enter_user
enter_user:
    mv sp, a0
    ld t0, 256(sp)
    csrw sepc, t0
    ld t0, 264(sp)
    csrw sstatus, t0
    la t0, user_trap
    csrw stvec, t0
    addi t0, sp, 272
    csrw sscratch, t0
    ld ra, 8(sp)
    ld gp, 24(sp)
    ld tp, 32(sp)
    ld t0, 40(sp)
    ld t1, 48(sp)
    ld t2, 56(sp)
    ld s0, 64(sp)
    ld s1, 72(sp)
    ld a0, 80(sp)
    ld a1, 88(sp)
    ld a2, 96(sp)
    ld a3, 104(sp)
    ld a4, 112(sp)
    ld a5, 120(sp)
    ld a6, 128(sp)
    ld a7, 136(sp)
    ld s2, 144(sp)
    ld s3, 152(sp)
    ld s4, 160(sp)
    ld s5, 168(sp)
    ld s6, 176(sp)
    ld s7, 184(sp)
    ld s8, 192(sp)
    ld s9, 200(sp)
    ld s10, 208(sp)
    ld s11, 216(sp)
    ld t3, 224(sp)
    ld t4, 232(sp)
    ld t5, 240(sp)
    ld t6, 248(sp)
    ld sp, 16(sp)
    sret

.align 4
.globl kernel_trap
kernel_trap:
    csrr a0, scause
    csrr a1, stval
    csrr a2, sepc
    call kernel_fault
4:  j 4b
