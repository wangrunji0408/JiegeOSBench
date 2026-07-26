//! Trap (exception and interrupt) handling.

pub mod context;

pub use context::{TaskContext, TrapContext};

use crate::arch::{self, scause, sstatus, stval};
use crate::syscall;
use crate::task;
use core::arch::global_asm;

global_asm!(include_str!("trap.S"));

extern "C" {
    fn __trap_entry();
    /// Return to user space with the given context. Never returns.
    pub fn __trap_return(cx: *mut TrapContext) -> !;
    /// Switch kernel context from `current` to `next`.
    pub fn __switch(current: *mut TaskContext, next: *const TaskContext);
}

/// Point `stvec` at our trap entry (direct mode).
pub fn init() {
    unsafe {
        core::arch::asm!("csrw stvec, {}", in(reg) __trap_entry as usize, options(nostack));
        // Allow the kernel to read and write user pages.
        sstatus::set_sum();
    }
}

/// Scause values for exceptions.
mod cause {
    pub const INST_MISALIGNED: usize = 0;
    pub const INST_FAULT: usize = 1;
    pub const ILLEGAL_INST: usize = 2;
    pub const BREAKPOINT: usize = 3;
    pub const LOAD_MISALIGNED: usize = 4;
    pub const LOAD_FAULT: usize = 5;
    pub const STORE_MISALIGNED: usize = 6;
    pub const STORE_FAULT: usize = 7;
    pub const ECALL_U: usize = 8;
    pub const INST_PAGE_FAULT: usize = 12;
    pub const LOAD_PAGE_FAULT: usize = 13;
    pub const STORE_PAGE_FAULT: usize = 15;

    pub const INT_SOFT: usize = 1;
    pub const INT_TIMER: usize = 5;
    pub const INT_EXT: usize = 9;
}

/// Entry point from `trap.S` for traps taken in user mode.
///
/// Returns the context to restore, which may belong to a different task if we
/// rescheduled.
#[no_mangle]
pub extern "C" fn trap_from_user(cx: &'static mut TrapContext) -> *mut TrapContext {
    let cause = scause::read();
    let is_interrupt = cause >> (usize::BITS - 1) == 1;
    let code = cause & !(1 << (usize::BITS - 1));

    if is_interrupt {
        handle_interrupt(code);
    } else {
        handle_exception(code, cx);
    }

    // Deliver any pending signals and honor reschedule requests before
    // returning to user space.
    task::handle_pending_signals(cx);
    task::check_reschedule();

    // The task may have been through `execve`, which replaces the context, so
    // re-read the current task's context pointer instead of reusing `cx`.
    task::current_trap_context()
}

fn handle_exception(code: usize, cx: &mut TrapContext) {
    match code {
        cause::ECALL_U => {
            // Advance past the `ecall` before dispatching, so that a syscall
            // which manipulates `sepc` (like `execve` or signal return) wins.
            cx.sepc += 4;
            let ret = syscall::dispatch(cx);
            // `execve` and `rt_sigreturn` set the return value themselves.
            if ret != syscall::SKIP_RETURN {
                cx.set_return(ret as usize);
            }
        }
        cause::INST_PAGE_FAULT | cause::LOAD_PAGE_FAULT | cause::STORE_PAGE_FAULT => {
            let addr = stval::read();
            let write = code == cause::STORE_PAGE_FAULT;
            let exec = code == cause::INST_PAGE_FAULT;
            let ok = {
                let task = task::current();
                let mut aspace = task.aspace.lock();
                aspace.handle_fault(addr, write, exec)
            };
            if !ok {
                let task = task::current();
                crate::warn!(
                    "SEGV pid={} tid={} {} fault at {:#x} pc={:#x}",
                    task.pid(),
                    task.tid,
                    if exec {
                        "instruction"
                    } else if write {
                        "store"
                    } else {
                        "load"
                    },
                    addr,
                    cx.sepc,
                );
                crate::task::dump_user_context(cx);
                task::force_signal(crate::signal::SIGSEGV);
            }
        }
        cause::ILLEGAL_INST => {
            crate::warn!(
                "illegal instruction at {:#x} (stval={:#x}) in pid={}",
                cx.sepc,
                stval::read(),
                task::current().pid()
            );
            task::force_signal(crate::signal::SIGILL);
        }
        cause::BREAKPOINT => {
            cx.sepc += 2;
        }
        cause::INST_MISALIGNED | cause::LOAD_MISALIGNED | cause::STORE_MISALIGNED => {
            crate::warn!(
                "misaligned access at {:#x} addr={:#x}",
                cx.sepc,
                stval::read()
            );
            task::force_signal(crate::signal::SIGBUS);
        }
        cause::INST_FAULT | cause::LOAD_FAULT | cause::STORE_FAULT => {
            crate::warn!(
                "access fault (code {}) at pc={:#x} addr={:#x}",
                code,
                cx.sepc,
                stval::read()
            );
            task::force_signal(crate::signal::SIGBUS);
        }
        _ => {
            crate::warn!("unhandled exception {} at {:#x}", code, cx.sepc);
            task::force_signal(crate::signal::SIGSEGV);
        }
    }
}

fn handle_interrupt(code: usize) {
    match code {
        cause::INT_TIMER => {
            crate::time::on_timer_tick();
            task::request_reschedule();
        }
        cause::INT_EXT => {
            crate::drivers::plic::handle_interrupt();
        }
        cause::INT_SOFT => {
            // Software interrupt: clear it. Used for IPI, which we don't need
            // on a single hart.
            unsafe { arch::sip::clear_ssoft() };
        }
        _ => crate::warn!("unhandled interrupt {}", code),
    }
}

/// Layout of the minimal frame `__kernel_trap` pushes.
#[repr(C)]
pub struct KernelTrapFrame {
    pub x: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
}

/// Entry point from `trap.S` for traps taken in supervisor mode.
#[no_mangle]
pub extern "C" fn trap_from_kernel(frame: &mut KernelTrapFrame) {
    let cause = scause::read();
    let is_interrupt = cause >> (usize::BITS - 1) == 1;
    let code = cause & !(1 << (usize::BITS - 1));

    if is_interrupt {
        handle_interrupt(code);
        return;
    }

    // A fault in the kernel while touching a user page is recoverable: this
    // happens when a syscall dereferences a lazily-mapped user buffer that our
    // `ensure` pass missed (for example after a concurrent unmap).
    if matches!(
        code,
        cause::LOAD_PAGE_FAULT | cause::STORE_PAGE_FAULT | cause::INST_PAGE_FAULT
    ) {
        let addr = stval::read();
        if crate::mm::is_user_addr(addr) && task::has_current() {
            let write = code == cause::STORE_PAGE_FAULT;
            let ok = {
                let task = task::current();
                let mut aspace = task.aspace.lock();
                aspace.handle_fault(addr, write, false)
            };
            if ok {
                return;
            }
        }
        panic!(
            "kernel page fault: code={} addr={:#x} pc={:#x}",
            code, addr, frame.sepc
        );
    }

    panic!(
        "kernel exception: code={} stval={:#x} pc={:#x} sp={:#x} ra={:#x}",
        code,
        stval::read(),
        frame.sepc,
        frame.x[2],
        frame.x[1],
    );
}

/// Enable supervisor interrupts.
#[inline]
pub fn enable_interrupts() {
    unsafe { sstatus::set_sie() };
}

/// Disable supervisor interrupts, returning the previous state.
#[inline]
pub fn disable_interrupts() -> bool {
    unsafe { sstatus::clear_sie() }
}

/// Restore a previously saved interrupt-enable state.
#[inline]
pub fn restore_interrupts(was_enabled: bool) {
    if was_enabled {
        enable_interrupts();
    }
}

/// Run `f` with interrupts disabled.
pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let was = disable_interrupts();
    let r = f();
    restore_interrupts(was);
    r
}
