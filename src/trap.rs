//! Trap handling and user-mode entry/exit.
//!
//! Design: no interrupts at all (sie = 0). The kernel polls devices while a
//! syscall blocks. Traps only come from user mode (ecall / faults) or are
//! fatal kernel bugs.
use core::arch::{asm, global_asm};

#[repr(C)]
#[derive(Clone)]
pub struct TrapFrame {
    pub regs: [usize; 32], // x0..x31 (x0 unused)
    pub sepc: usize,       // offset 256
    pub sstatus: usize,    // offset 264
    kernel_ra: usize,      // offset 272
    kernel_sp: usize,      // offset 280
    kernel_s: [usize; 12], // offset 288..384
}

impl TrapFrame {
    pub fn new() -> Self {
        // SPP=0 (user), SPIE=1, SUM=1, FS=Initial(01)
        let sstatus = read_sstatus() & !(1 << 8) | (1 << 5) | (1 << 18) | (1 << 13);
        TrapFrame {
            regs: [0; 32],
            sepc: 0,
            sstatus,
            kernel_ra: 0,
            kernel_sp: 0,
            kernel_s: [0; 12],
        }
    }
    pub fn sp(&mut self) -> &mut usize {
        &mut self.regs[2]
    }
    pub fn syscall_args(&self) -> [usize; 6] {
        [
            self.regs[10],
            self.regs[11],
            self.regs[12],
            self.regs[13],
            self.regs[14],
            self.regs[15],
        ]
    }
    pub fn syscall_nr(&self) -> usize {
        self.regs[17]
    }
    pub fn set_ret(&mut self, v: usize) {
        self.regs[10] = v;
    }
}

global_asm!(
    r#"
    .align 2
    .globl __trap_entry
__trap_entry:
    csrrw a0, sscratch, a0      # a0 = &TrapFrame (or 0 if trap from kernel)
    beqz a0, 9f
    sd x1, 8(a0)
    sd x2, 16(a0)
    sd x3, 24(a0)
    sd x4, 32(a0)
    sd x5, 40(a0)
    sd x6, 48(a0)
    sd x7, 56(a0)
    sd x8, 64(a0)
    sd x9, 72(a0)
    sd x11, 88(a0)
    sd x12, 96(a0)
    sd x13, 104(a0)
    sd x14, 112(a0)
    sd x15, 120(a0)
    sd x16, 128(a0)
    sd x17, 136(a0)
    sd x18, 144(a0)
    sd x19, 152(a0)
    sd x20, 160(a0)
    sd x21, 168(a0)
    sd x22, 176(a0)
    sd x23, 184(a0)
    sd x24, 192(a0)
    sd x25, 200(a0)
    sd x26, 208(a0)
    sd x27, 216(a0)
    sd x28, 224(a0)
    sd x29, 232(a0)
    sd x30, 240(a0)
    sd x31, 248(a0)
    csrr t0, sscratch
    sd t0, 80(a0)               # user a0
    csrw sscratch, x0           # mark: now in kernel
    csrr t0, sepc
    sd t0, 256(a0)
    csrr t0, sstatus
    sd t0, 264(a0)
    # restore kernel context and return into __run_user's caller
    ld ra, 272(a0)
    ld sp, 280(a0)
    ld s0, 288(a0)
    ld s1, 296(a0)
    ld s2, 304(a0)
    ld s3, 312(a0)
    ld s4, 320(a0)
    ld s5, 328(a0)
    ld s6, 336(a0)
    ld s7, 344(a0)
    ld s8, 352(a0)
    ld s9, 360(a0)
    ld s10, 368(a0)
    ld s11, 376(a0)
    ret
9:  # trap from kernel mode: restore a0 and panic
    csrrw a0, sscratch, a0
    call kernel_trap_panic
1:  j 1b

    .globl __run_user
__run_user:                     # a0 = &TrapFrame
    sd ra, 272(a0)
    sd sp, 280(a0)
    sd s0, 288(a0)
    sd s1, 296(a0)
    sd s2, 304(a0)
    sd s3, 312(a0)
    sd s4, 320(a0)
    sd s5, 328(a0)
    sd s6, 336(a0)
    sd s7, 344(a0)
    sd s8, 352(a0)
    sd s9, 360(a0)
    sd s10, 368(a0)
    sd s11, 376(a0)
    csrw sscratch, a0
    ld t0, 264(a0)
    csrw sstatus, t0
    ld t0, 256(a0)
    csrw sepc, t0
    ld x1, 8(a0)
    ld x2, 16(a0)
    ld x3, 24(a0)
    ld x4, 32(a0)
    ld x5, 40(a0)
    ld x6, 48(a0)
    ld x7, 56(a0)
    ld x8, 64(a0)
    ld x9, 72(a0)
    ld x11, 88(a0)
    ld x12, 96(a0)
    ld x13, 104(a0)
    ld x14, 112(a0)
    ld x15, 120(a0)
    ld x16, 128(a0)
    ld x17, 136(a0)
    ld x18, 144(a0)
    ld x19, 152(a0)
    ld x20, 160(a0)
    ld x21, 168(a0)
    ld x22, 176(a0)
    ld x23, 184(a0)
    ld x24, 192(a0)
    ld x25, 200(a0)
    ld x26, 208(a0)
    ld x27, 216(a0)
    ld x28, 224(a0)
    ld x29, 232(a0)
    ld x30, 240(a0)
    ld x31, 248(a0)
    ld a0, 80(a0)
    sret
"#
);

extern "C" {
    fn __trap_entry();
    fn __run_user(tf: *mut TrapFrame);
}

pub fn init() {
    unsafe {
        asm!("csrw stvec, {}", in(reg) __trap_entry as usize);
        // disable all interrupt sources; we poll
        asm!("csrw sie, zero");
        // SUM so kernel code can touch user pages
        asm!("csrs sstatus, {}", in(reg) 1usize << 18);
    }
}

/// Enter user mode; returns when the user traps.
pub fn run_user(tf: &mut TrapFrame) -> (usize, usize) {
    unsafe { __run_user(tf as *mut _) };
    (read_scause(), read_stval())
}

fn read_sstatus() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, sstatus", out(reg) v) };
    v
}
fn read_scause() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, scause", out(reg) v) };
    v
}
fn read_stval() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, stval", out(reg) v) };
    v
}
pub fn read_sepc() -> usize {
    let v: usize;
    unsafe { asm!("csrr {}, sepc", out(reg) v) };
    v
}

#[no_mangle]
extern "C" fn kernel_trap_panic() -> ! {
    panic!(
        "trap from kernel: scause={:#x} stval={:#x} sepc={:#x}",
        read_scause(),
        read_stval(),
        read_sepc()
    );
}

pub const SCAUSE_ECALL_U: usize = 8;
pub const SCAUSE_IFAULT: usize = 12;
pub const SCAUSE_LFAULT: usize = 13;
pub const SCAUSE_SFAULT: usize = 15;
