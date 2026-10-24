use core::arch::{asm, global_asm};

pub const SCAUSE_INTERRUPT: usize = 1usize << 63;

global_asm!(
    r#"
    .section .text.boot
    .globl _start
    .type _start, @function
_start:
    csrw sie, zero
    csrw sip, zero
    la sp, boot_stack_top
    la t0, __bss_start
    la t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    call rust_main
3:
    wfi
    j 3b

    .section .bss.stack
    .align 4
    .globl boot_stack_bottom
boot_stack_bottom:
    .skip 131072
    .globl boot_stack_top
boot_stack_top:
"#
);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    pub scause: usize,
    pub stval: usize,
}

global_asm!(
    r#"
    .section .text
    .align 2
    .globl kernel_trap_entry
kernel_trap_entry:
    addi sp, sp, -288
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
    csrr t0, sstatus
    sd t0, 256(sp)
    csrr t0, sepc
    sd t0, 264(sp)
    csrr t0, scause
    sd t0, 272(sp)
    csrr t0, stval
    sd t0, 280(sp)
    mv a0, sp
    call kernel_trap_handler
    ld t0, 264(sp)
    csrw sepc, t0
    ld t0, 256(sp)
    csrw sstatus, t0
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
    addi sp, sp, 288
    sret

    .align 2
    .globl user_trap_entry
user_trap_entry:
    csrrw sp, sscratch, sp
    addi sp, sp, -288
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
    csrw sscratch, zero
    csrr t0, sstatus
    sd t0, 256(sp)
    csrr t0, sepc
    sd t0, 264(sp)
    csrr t0, scause
    sd t0, 272(sp)
    csrr t0, stval
    sd t0, 280(sp)
    la t0, kernel_trap_entry
    csrw stvec, t0
    mv a0, sp
    call user_trap_handler
    mv a0, sp
    call user_restore

    .align 2
    .globl user_restore
user_restore:
    mv sp, a0
    la t0, user_trap_entry
    csrw stvec, t0
    la t0, boot_stack_top
    csrw sscratch, t0
    ld t0, 264(sp)
    csrw sepc, t0
    ld t0, 256(sp)
    csrw sstatus, t0
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
"#
);

unsafe extern "C" {
    fn kernel_trap_entry();
    fn user_restore(frame: usize) -> !;
}

pub fn init() {
    unsafe {
        asm!(
            "csrw stvec, {}",
            in(reg) kernel_trap_entry as *const () as usize
        );
        asm!("csrw sie, {}", in(reg) (1usize << 5) | (1usize << 9));
    }
    arm_timer();
}

pub fn time() -> u64 {
    let value: u64;
    unsafe { asm!("rdtime {}", out(reg) value) };
    value
}

pub fn arm_timer() {
    crate::sbi::set_timer(time() + 100_000);
}

pub fn enable_interrupts() {
    unsafe { asm!("csrs sstatus, {}", in(reg) 1usize << 1) };
}

pub fn disable_interrupts() {
    unsafe { asm!("csrc sstatus, {}", in(reg) 1usize << 1) };
}

#[unsafe(no_mangle)]
extern "C" fn kernel_trap_handler(frame: &mut TrapFrame) {
    dispatch(frame, false);
}

#[unsafe(no_mangle)]
extern "C" fn user_trap_handler(frame: &mut TrapFrame) {
    dispatch(frame, true);
}

fn dispatch(frame: &mut TrapFrame, from_user: bool) {
    if frame.scause & SCAUSE_INTERRUPT != 0 {
        match frame.scause & !SCAUSE_INTERRUPT {
            5 => {
                crate::drivers::timer_interrupt();
                arm_timer();
            }
            9 => crate::drivers::external_interrupt(),
            code => panic!("unknown interrupt {code}: {frame:#x?}"),
        }
        return;
    }
    if from_user && frame.scause == 8 {
        frame.sepc += 4;
        crate::syscall::dispatch(frame);
        return;
    }
    panic!(
        "trap from_user={from_user} cause={:#x} sepc={:#x} stval={:#x}\n{frame:#x?}",
        frame.scause, frame.sepc, frame.stval
    );
}

pub fn enter_user(entry: usize, stack: usize) -> ! {
    let mut frame: TrapFrame = unsafe { core::mem::zeroed() };
    frame.regs[2] = stack;
    frame.sepc = entry;
    frame.sstatus = (1 << 5) | (1 << 13);
    unsafe { user_restore(&raw const frame as usize) }
}

#[inline]
pub fn wait_for_interrupt() {
    unsafe { asm!("wfi", options(nomem, nostack)) }
}
