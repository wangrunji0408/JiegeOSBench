//! Kernel-thread context: callee-saved registers only. Switching between
//! tasks happens purely in kernel space via `__switch` (`switch.S`); the
//! trampoline handles the separate user<->kernel boundary.

use crate::trap::trap_return;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskContext {
    ra: usize,
    sp: usize,
    s: [usize; 12],
}

impl TaskContext {
    pub fn zero_init() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }

    /// Context for a brand new task: switching into it for the first time
    /// will "return" into `trap_return`, which restores the initial
    /// `TrapContext` and drops into user mode at the ELF entry point.
    pub fn goto_trap_return(kstack_sp: usize) -> Self {
        Self {
            ra: trap_return as usize,
            sp: kstack_sp,
            s: [0; 12],
        }
    }
}

unsafe extern "C" {
    pub fn __switch(current_task_cx_ptr: *mut TaskContext, next_task_cx_ptr: *const TaskContext);
}
