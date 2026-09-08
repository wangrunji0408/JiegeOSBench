//! Trap entry/exit and dispatch.

use crate::csr::*;
use crate::{print, println};

/// Layout must match trap.S exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub x: [usize; 32], // 0..256
    pub sepc: usize,    // 256
    pub sstatus: usize, // 264
}

pub const TF_SIZE: usize = 288;

impl TrapFrame {
    #[inline]
    pub fn a0(&self) -> usize {
        self.x[10]
    }
    #[inline]
    pub fn a1(&self) -> usize {
        self.x[11]
    }
    #[inline]
    pub fn a2(&self) -> usize {
        self.x[12]
    }
    #[inline]
    pub fn a3(&self) -> usize {
        self.x[13]
    }
    #[inline]
    pub fn a7(&self) -> usize {
        self.x[17]
    }
    #[inline]
    pub fn sp(&self) -> usize {
        self.x[2]
    }
    #[inline]
    pub fn ra(&self) -> usize {
        self.x[1]
    }
    #[inline]
    pub fn set_a0(&mut self, v: usize) {
        self.x[10] = v;
    }
    #[inline]
    pub fn set_sp(&mut self, v: usize) {
        self.x[2] = v;
    }
    #[inline]
    pub fn set_ra(&mut self, v: usize) {
        self.x[1] = v;
    }
    #[inline]
    pub fn from_user(&self) -> bool {
        self.sstatus & SSTATUS_SPP == 0
    }
}

extern "C" {
    fn __trap_entry();
    fn __restore(tf: *const TrapFrame) -> !;
    pub fn __switch(prev: *mut TaskContext, next: *const TaskContext);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TaskContext {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

impl TaskContext {
    pub const fn empty() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }
}

/// Point stvec at the trap entry.
pub fn init() {
    set_stvec(__trap_entry as usize);
    // Make the kernel use the same page table (already active from boot).
    unsafe {
        set_sie(SIE_SEIE | SIE_STIE);
    }
}

pub fn enter_user(tf: &TrapFrame) -> ! {
    unsafe { __restore(tf) }
}

/// Jump to a freshly built trap frame from a kernel task context.
pub unsafe fn restore_from(tf: *const TrapFrame) -> ! {
    __restore(tf)
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let scause = csrr!("scause");
    if !tf.from_user() {
        kernel_trap(tf, scause);
        return;
    }
    crate::task::user_trap(tf, scause);
}

fn kernel_trap(tf: &mut TrapFrame, scause: usize) -> ! {
    let stval = csrr!("stval");
    println!("\n=== FATAL: kernel trap ===");
    println!("scause={:#x} ({}) stval={:#x} sepc={:#x}", scause, cause_name(scause), stval, tf.sepc);
    println!("ra={:#x} sp={:#x}", tf.ra(), tf.sp());
    panic!("kernel trap");
}

pub fn cause_name(scause: usize) -> &'static str {
    match scause {
        SCAUSE_INST_MISALIGNED => "instruction address misaligned",
        SCAUSE_INST_ACCESS => "instruction access fault",
        SCAUSE_ILLEGAL_INST => "illegal instruction",
        SCAUSE_BREAKPOINT => "breakpoint",
        SCAUSE_LOAD_MISALIGNED => "load address misaligned",
        SCAUSE_LOAD_ACCESS => "load access fault",
        SCAUSE_STORE_MISALIGNED => "store address misaligned",
        SCAUSE_STORE_ACCESS => "store access fault",
        SCAUSE_ECALL_U => "ecall from U",
        SCAUSE_ECALL_S => "ecall from S",
        SCAUSE_INST_PAGE_FAULT => "instruction page fault",
        SCAUSE_LOAD_PAGE_FAULT => "load page fault",
        SCAUSE_STORE_PAGE_FAULT => "store page fault",
        SCAUSE_TIMER => "supervisor timer interrupt",
        SCAUSE_EXTERNAL => "supervisor external interrupt",
        _ => "unknown",
    }
}
