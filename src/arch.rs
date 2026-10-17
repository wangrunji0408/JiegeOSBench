// RISC-V privileged architecture helpers: CSRs, trap entry/exit.
use core::arch::{asm, global_asm};

pub const SSTATUS_SIE: usize = 1 << 1;
pub const SSTATUS_SPIE: usize = 1 << 5;
pub const SSTATUS_SPP: usize = 1 << 8;
pub const SSTATUS_FS: usize = 3 << 13;
pub const SSTATUS_SUM: usize = 1 << 18;

#[repr(C)]
pub struct TrapFrame {
    pub x: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
}

impl TrapFrame {
    pub fn a(&self, i: usize) -> usize {
        self.x[10 + i]
    }
    pub fn set_ret(&mut self, v: isize) {
        self.x[10] = v as usize;
    }
}

global_asm!(
    r#"
    .section .text
    .align 4
    .global trap_entry
    .type trap_entry, @function
trap_entry:
    csrrw   sp, sscratch, sp
    addi    sp, sp, -272
    sd      x1, 8(sp)
    sd      x3, 24(sp)
    sd      x4, 32(sp)
    sd      x5, 40(sp)
    sd      x6, 48(sp)
    sd      x7, 56(sp)
    sd      x8, 64(sp)
    sd      x9, 72(sp)
    sd      x10, 80(sp)
    sd      x11, 88(sp)
    sd      x12, 96(sp)
    sd      x13, 104(sp)
    sd      x14, 112(sp)
    sd      x15, 120(sp)
    sd      x16, 128(sp)
    sd      x17, 136(sp)
    sd      x18, 144(sp)
    sd      x19, 152(sp)
    sd      x20, 160(sp)
    sd      x21, 168(sp)
    sd      x22, 176(sp)
    sd      x23, 184(sp)
    sd      x24, 192(sp)
    sd      x25, 200(sp)
    sd      x26, 208(sp)
    sd      x27, 216(sp)
    sd      x28, 224(sp)
    sd      x29, 232(sp)
    sd      x30, 240(sp)
    sd      x31, 248(sp)
    csrr    t0, sepc
    sd      t0, 256(sp)
    csrr    t0, sstatus
    sd      t0, 264(sp)
    mv      a0, sp
    call    trap_handler
    ld      t0, 256(sp)
    csrw    sepc, t0
    ld      t0, 264(sp)
    csrw    sstatus, t0
    ld      x1, 8(sp)
    ld      x3, 24(sp)
    ld      x4, 32(sp)
    ld      x5, 40(sp)
    ld      x6, 48(sp)
    ld      x7, 56(sp)
    ld      x8, 64(sp)
    ld      x9, 72(sp)
    ld      x10, 80(sp)
    ld      x11, 88(sp)
    ld      x12, 96(sp)
    ld      x13, 104(sp)
    ld      x14, 112(sp)
    ld      x15, 120(sp)
    ld      x16, 128(sp)
    ld      x17, 136(sp)
    ld      x18, 144(sp)
    ld      x19, 152(sp)
    ld      x20, 160(sp)
    ld      x21, 168(sp)
    ld      x22, 176(sp)
    ld      x23, 184(sp)
    ld      x24, 192(sp)
    ld      x25, 200(sp)
    ld      x26, 208(sp)
    ld      x27, 216(sp)
    ld      x28, 224(sp)
    ld      x29, 232(sp)
    ld      x30, 240(sp)
    ld      x31, 248(sp)
    addi    sp, sp, 272
    csrrw   sp, sscratch, sp
    sret
"#
);

static mut TRAP_STACK: [u8; 65536] = [0; 65536];

pub fn init_trap() {
    unsafe {
        let top = core::ptr::addr_of_mut!(TRAP_STACK) as usize + 65536;
        let entry = trap_entry as usize;
        asm!("csrw stvec, {}", in(reg) entry);
        asm!("csrw sscratch, {}", in(reg) top);
    }
}

extern "C" {
    fn trap_entry();
}

#[inline]
pub fn sepc() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, sepc", out(reg) v) };
    v
}

#[inline]
pub fn scause() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, scause", out(reg) v) };
    v
}

#[inline]
pub fn stval() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, stval", out(reg) v) };
    v
}

#[inline]
pub fn rdtime() -> u64 {
    let v: u64;
    unsafe { asm!("rdtime {}", out(reg) v) };
    v
}

#[inline]
pub fn fence_i() {
    unsafe { asm!("fence.i") };
}

/// Enter user mode at `entry` with stack pointer `sp`. Never returns.
pub unsafe fn enter_user(entry: usize, sp: usize) -> ! {
    let status = SSTATUS_SPIE | SSTATUS_FS | SSTATUS_SUM;
    asm!(
        "csrw sepc, {entry}",
        "csrw sstatus, {status}",
        "mv sp, {sp}",
        "sret",
        entry = in(reg) entry,
        status = in(reg) status,
        sp = in(reg) sp,
        options(noreturn)
    );
}
