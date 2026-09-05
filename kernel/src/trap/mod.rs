//! Trap entry/exit, interrupt dispatch.
use core::arch::global_asm;

use crate::abi::*;
use crate::mm::addrspace::{AccessKind, FaultError};
use crate::task::{current, sched, signal};

global_asm!(include_str!("trap.S"));

extern "C" {
    pub fn __uservec();
    pub fn __userret(tf: *mut TrapFrame) -> !;
    pub fn __kernelvec();
    pub fn __switch(cur: *mut Context, next: *const Context);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub x: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
    pub kernel_sp: usize,
    pub f: [u64; 32],
    pub fcsr: usize,
}

impl TrapFrame {
    pub const SSTATUS_SPIE: usize = 1 << 5;
    pub const SSTATUS_SPP: usize = 1 << 8;
    pub const SSTATUS_FS_INITIAL: usize = 1 << 13;
    pub const SSTATUS_SUM: usize = 1 << 18;

    /// Fresh user-mode frame.
    pub fn new_user(entry: usize, sp: usize, kernel_sp: usize) -> Self {
        let mut tf = TrapFrame::default();
        tf.sepc = entry;
        tf.x[2] = sp;
        tf.sstatus = Self::SSTATUS_SPIE | Self::SSTATUS_FS_INITIAL | Self::SSTATUS_SUM;
        tf.kernel_sp = kernel_sp;
        tf
    }

    #[inline]
    pub fn a0(&self) -> usize {
        self.x[10]
    }
    #[inline]
    pub fn set_a0(&mut self, v: usize) {
        self.x[10] = v;
    }
    #[inline]
    pub fn sp(&self) -> usize {
        self.x[2]
    }
    #[inline]
    pub fn set_sp(&mut self, v: usize) {
        self.x[2] = v;
    }
    pub fn syscall_args(&self) -> [usize; 6] {
        [self.x[10], self.x[11], self.x[12], self.x[13], self.x[14], self.x[15]]
    }
}

/// Callee-saved register context for kernel context switches.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
    pub fs: [u64; 12],
}

impl Context {
    pub const fn zero() -> Self {
        Context { ra: 0, sp: 0, s: [0; 12], fs: [0; 12] }
    }
}

#[repr(C)]
struct KernelTrapFrame {
    x: [usize; 32],
    sepc: usize,
    sstatus: usize,
}

pub mod csr {
    #[inline]
    pub fn read_scause() -> usize {
        let v: usize;
        unsafe { core::arch::asm!("csrr {}, scause", out(reg) v) };
        v
    }
    #[inline]
    pub fn read_stval() -> usize {
        let v: usize;
        unsafe { core::arch::asm!("csrr {}, stval", out(reg) v) };
        v
    }
    #[inline]
    pub fn read_sstatus() -> usize {
        let v: usize;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) v) };
        v
    }
    #[inline]
    pub fn read_sepc() -> usize {
        let v: usize;
        unsafe { core::arch::asm!("csrr {}, sepc", out(reg) v) };
        v
    }
    #[inline]
    pub fn write_stvec(v: usize) {
        unsafe { core::arch::asm!("csrw stvec, {}", in(reg) v) };
    }
    #[inline]
    pub fn write_sscratch(v: usize) {
        unsafe { core::arch::asm!("csrw sscratch, {}", in(reg) v) };
    }
    #[inline]
    pub fn write_satp(v: usize) {
        unsafe { core::arch::asm!("csrw satp, {}", in(reg) v) };
    }
    #[inline]
    pub fn read_satp() -> usize {
        let v: usize;
        unsafe { core::arch::asm!("csrr {}, satp", out(reg) v) };
        v
    }
    #[inline]
    pub fn set_sie_bits(v: usize) {
        unsafe { core::arch::asm!("csrs sie, {}", in(reg) v) };
    }
    #[inline]
    pub fn enable_interrupts() {
        unsafe { core::arch::asm!("csrsi sstatus, 2") };
    }
    #[inline]
    pub fn disable_interrupts() {
        unsafe { core::arch::asm!("csrci sstatus, 2") };
    }
    #[inline]
    pub fn read_time() -> u64 {
        let v: u64;
        unsafe { core::arch::asm!("rdtime {}", out(reg) v) };
        v
    }
    #[inline]
    pub fn set_fs_initial() {
        unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) 1usize << 13) };
    }
    #[inline]
    pub fn wfi() {
        unsafe { core::arch::asm!("wfi") };
    }
}

pub const SIE_SSIE: usize = 1 << 1;
pub const SIE_STIE: usize = 1 << 5;
pub const SIE_SEIE: usize = 1 << 9;

const CAUSE_INTERRUPT: usize = 1 << 63;
const IRQ_S_TIMER: usize = 5;
const IRQ_S_EXT: usize = 9;

pub fn init() {
    csr::write_stvec(__kernelvec as *const () as usize);
    csr::set_fs_initial();
    csr::set_sie_bits(SIE_STIE | SIE_SEIE);
}

fn handle_interrupt(cause: usize, from_user: bool) {
    match cause & !CAUSE_INTERRUPT {
        IRQ_S_TIMER => {
            crate::time::on_timer_irq();
            if from_user {
                sched::yield_now();
            }
        }
        IRQ_S_EXT => {
            crate::drivers::plic::handle_irq();
        }
        other => panic!("unknown interrupt {:#x}", other),
    }
}

/// Entry from `__uservec` (on the task's kernel stack).
#[no_mangle]
pub extern "C" fn user_trap_handler(tf: &mut TrapFrame) -> ! {
    let scause = csr::read_scause();
    let stval = csr::read_stval();
    let task = current();
    task.stats_enter_kernel();

    if scause & CAUSE_INTERRUPT != 0 {
        handle_interrupt(scause, true);
    } else {
        match scause {
            8 => {
                // U-mode ecall
                tf.sepc += 4;
                crate::syscall::dispatch(tf);
            }
            12 | 13 | 15 => {
                let kind = match scause {
                    12 => AccessKind::Exec,
                    13 => AccessKind::Read,
                    _ => AccessKind::Write,
                };
                let res = task.mm().lock().handle_fault(stval, kind);
                if let Err(e) = res {
                    let sig = match e {
                        FaultError::NoMapping | FaultError::Protection => SIGSEGV,
                        FaultError::Io => SIGBUS,
                    };
                    klog!(
                        "pid {} ({}): page fault {:?} at {:#x} (sepc={:#x}, sp={:#x}) -> {:?}",
                        task.pid,
                        task.name(),
                        kind,
                        stval,
                        tf.sepc,
                        tf.sp(),
                        e
                    );
                    if crate::config::KLOG {
                        task.mm().lock().dump();
                    }
                    signal::send_signal(&task, sig, None);
                }
            }
            2 => {
                klog!("pid {}: illegal instruction at {:#x} (stval={:#x})", task.pid, tf.sepc, stval);
                signal::send_signal(&task, SIGILL, None);
            }
            3 => {
                signal::send_signal(&task, SIGTRAP, None);
            }
            0 | 4 | 6 => {
                klog!("pid {}: misaligned access cause={} at {:#x} (stval={:#x})", task.pid, scause, tf.sepc, stval);
                signal::send_signal(&task, SIGBUS, None);
            }
            1 | 5 | 7 => {
                klog!("pid {}: access fault cause={} at {:#x} (stval={:#x})", task.pid, scause, tf.sepc, stval);
                signal::send_signal(&task, SIGSEGV, None);
            }
            other => panic!("unhandled user trap scause={:#x} stval={:#x} sepc={:#x}", other, stval, tf.sepc),
        }
    }
    return_to_user()
}

/// Deliver pending signals and return to user mode for the current task.
pub fn return_to_user() -> ! {
    let task = current();
    loop {
        signal::deliver_pending(&task);
        // deliver_pending may have killed us (never returns) or blocked; re-check
        if !signal::has_deliverable(&task) {
            break;
        }
    }
    task.stats_leave_kernel();
    let tf = task.tf_ptr();
    unsafe { __userret(tf) }
}

#[no_mangle]
extern "C" fn kernel_trap_handler(tf: &mut KernelTrapFrame) {
    let scause = csr::read_scause();
    let stval = csr::read_stval();
    if scause & CAUSE_INTERRUPT != 0 {
        handle_interrupt(scause, false);
        return;
    }
    panic!(
        "kernel trap: scause={:#x} stval={:#x} sepc={:#x} sstatus={:#x} sp={:#x} ra={:#x}",
        scause, stval, tf.sepc, tf.sstatus, tf.x[2], tf.x[1]
    );
}
