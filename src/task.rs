//! 任务上下文与切换原语。

use core::arch::global_asm;
use crate::mm::PAGE_SIZE;

global_asm!(include_str!("switch.S"));

/// 进程内核栈大小
pub const KSTACK_SIZE: usize = PAGE_SIZE * 4; // 16KB

/// 任务上下文，与 switch.S 布局对应：ra, sp, s0..s11
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskContext {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

impl TaskContext {
    pub const fn zero() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
pub enum TaskState {
    Ready,
    Running,
    Exited,
}

extern "C" {
    fn __switch(cur: *mut TaskContext, next: *const TaskContext);
}

/// 切换到 next 任务。保存当前上下文到 cur。
pub unsafe fn switch_to(cur: *mut TaskContext, next: *const TaskContext) {
    __switch(cur, next);
}
