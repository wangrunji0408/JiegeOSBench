use core::arch::global_asm;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapContext {
    pub x: [usize; 32], // x[0] unused; x[2]=sp
    pub sepc: usize,    // index 32
    pub sstatus: usize, // index 33
}

impl TrapContext {
    pub fn a(&self, i: usize) -> usize {
        self.x[10 + i]
    }
    pub fn set_ret(&mut self, v: usize) {
        self.x[10] = v;
    }
    pub fn syscall_no(&self) -> usize {
        self.x[17]
    }
}

global_asm!(
    ".align 4",
    ".globl __alltraps",
    "__alltraps:",
    "   csrrw sp, sscratch, sp",   // sp = kernel stack top, sscratch = user sp
    "   addi sp, sp, -34*8",
    // save x1, x3..x31 (skip x0 and x2/sp which we handle via sscratch)
    "   sd x1, 1*8(sp)",
    "   sd x3, 3*8(sp)",
    "   sd x4, 4*8(sp)",
    "   sd x5, 5*8(sp)",
    "   sd x6, 6*8(sp)",
    "   sd x7, 7*8(sp)",
    "   sd x8, 8*8(sp)",
    "   sd x9, 9*8(sp)",
    "   sd x10, 10*8(sp)",
    "   sd x11, 11*8(sp)",
    "   sd x12, 12*8(sp)",
    "   sd x13, 13*8(sp)",
    "   sd x14, 14*8(sp)",
    "   sd x15, 15*8(sp)",
    "   sd x16, 16*8(sp)",
    "   sd x17, 17*8(sp)",
    "   sd x18, 18*8(sp)",
    "   sd x19, 19*8(sp)",
    "   sd x20, 20*8(sp)",
    "   sd x21, 21*8(sp)",
    "   sd x22, 22*8(sp)",
    "   sd x23, 23*8(sp)",
    "   sd x24, 24*8(sp)",
    "   sd x25, 25*8(sp)",
    "   sd x26, 26*8(sp)",
    "   sd x27, 27*8(sp)",
    "   sd x28, 28*8(sp)",
    "   sd x29, 29*8(sp)",
    "   sd x30, 30*8(sp)",
    "   sd x31, 31*8(sp)",
    // save user sp (from sscratch) into x[2]
    "   csrr t0, sscratch",
    "   sd t0, 2*8(sp)",
    // save sepc, sstatus
    "   csrr t0, sepc",
    "   sd t0, 32*8(sp)",
    "   csrr t0, sstatus",
    "   sd t0, 33*8(sp)",
    // restore sscratch to kernel stack top for the next trap
    "   addi t0, sp, 34*8",
    "   csrw sscratch, t0",
    "   mv a0, sp",
    "   call trap_handler",
    // fallthrough into restore with a0 = context pointer
    ".globl __restore",
    "__restore:",
    "   mv sp, a0",
    "   ld t0, 32*8(sp)",
    "   csrw sepc, t0",
    "   ld t0, 33*8(sp)",
    "   csrw sstatus, t0",
    // set sscratch = kernel stack top (sp + 34*8) for next trap
    "   addi t0, sp, 34*8",
    "   csrw sscratch, t0",
    "   ld x1, 1*8(sp)",
    "   ld x3, 3*8(sp)",
    "   ld x4, 4*8(sp)",
    "   ld x5, 5*8(sp)",
    "   ld x6, 6*8(sp)",
    "   ld x7, 7*8(sp)",
    "   ld x8, 8*8(sp)",
    "   ld x9, 9*8(sp)",
    "   ld x10, 10*8(sp)",
    "   ld x11, 11*8(sp)",
    "   ld x12, 12*8(sp)",
    "   ld x13, 13*8(sp)",
    "   ld x14, 14*8(sp)",
    "   ld x15, 15*8(sp)",
    "   ld x16, 16*8(sp)",
    "   ld x17, 17*8(sp)",
    "   ld x18, 18*8(sp)",
    "   ld x19, 19*8(sp)",
    "   ld x20, 20*8(sp)",
    "   ld x21, 21*8(sp)",
    "   ld x22, 22*8(sp)",
    "   ld x23, 23*8(sp)",
    "   ld x24, 24*8(sp)",
    "   ld x25, 25*8(sp)",
    "   ld x26, 26*8(sp)",
    "   ld x27, 27*8(sp)",
    "   ld x28, 28*8(sp)",
    "   ld x29, 29*8(sp)",
    "   ld x30, 30*8(sp)",
    "   ld x31, 31*8(sp)",
    "   ld x2, 2*8(sp)",   // restore user sp last
    "   sret",
);

extern "C" {
    pub fn __alltraps();
    pub fn __restore(cx: *mut TrapContext) -> !;
}

pub fn init() {
    unsafe {
        core::arch::asm!("csrw stvec, {}", in(reg) __alltraps as usize);
        // No interrupts: fully cooperative kernel. Ensure sie disabled.
        core::arch::asm!("csrw sie, zero");
    }
}

fn read_csr(_name: &str) -> usize {
    0
}

#[no_mangle]
pub extern "C" fn trap_handler(cx: &mut TrapContext) -> *mut TrapContext {
    let scause: usize;
    let stval: usize;
    unsafe {
        core::arch::asm!("csrr {}, scause", out(reg) scause);
        core::arch::asm!("csrr {}, stval", out(reg) stval);
    }
    let is_interrupt = scause >> 63 == 1;
    let code = scause & 0xfff;
    if !is_interrupt && code == 8 {
        // Environment call from U-mode (syscall).
        cx.sepc += 4;
        crate::syscall::dispatch(cx);
    } else if is_interrupt {
        // We run interrupt-free; ignore any spurious interrupt.
    } else {
        crate::println!(
            "[kernel] FATAL trap: scause={:#x} stval={:#x} sepc={:#x}",
            scause,
            stval,
            cx.sepc
        );
        crate::println!("[kernel] user faulted; shutting down");
        crate::sbi::shutdown();
    }
    cx as *mut TrapContext
}
