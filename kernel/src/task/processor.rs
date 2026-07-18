//! Single-hart scheduler core: tracks the currently running task and the
//! "idle" context that the scheduling loop itself runs on.

use super::context::TaskContext;
use super::manager::{add_task, fetch_task};
use super::task::{TaskControlBlock, TaskStatus};
use crate::task::context::__switch;
use crate::trap::TrapContext;
use alloc::sync::Arc;
use spin::Mutex;

struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: TaskContext,
}

impl Processor {
    const fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
        }
    }
    fn idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }
}

static PROCESSOR: Mutex<Processor> = Mutex::new(Processor::new());

pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.lock().current.take()
}

pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.lock().current.clone()
}

pub fn current_user_token() -> usize {
    current_task().unwrap().inner_lock().user_token()
}

pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task().unwrap().inner_lock().trap_cx()
}

/// Scheduling loop: repeatedly pick a ready task and switch into it. This
/// function's own stack frame becomes the "idle" kernel thread; tasks
/// switch back into it (via `schedule`) when they block, yield, or exit.
pub fn run_tasks() -> ! {
    loop {
        if let Some(task) = fetch_task() {
            let idle_task_cx_ptr = PROCESSOR.lock().idle_task_cx_ptr();
            let next_task_cx_ptr = {
                let mut inner = task.inner_lock();
                inner.task_status = TaskStatus::Running;
                &inner.task_cx as *const TaskContext
            };
            PROCESSOR.lock().current = Some(task);
            unsafe {
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            }
        } else {
            core::hint::spin_loop();
        }
    }
}

/// Switch from the currently running task's context back to the idle loop.
/// Called with the task's own lock already released.
pub fn schedule(switched_task_cx_ptr: *mut TaskContext) {
    let idle_task_cx_ptr = PROCESSOR.lock().idle_task_cx_ptr();
    unsafe {
        __switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}

pub fn suspend_current_and_run_next() {
    let task = take_current_task().unwrap();
    let task_cx_ptr = {
        let mut inner = task.inner_lock();
        inner.task_status = TaskStatus::Ready;
        &mut inner.task_cx as *mut TaskContext
    };
    add_task(task);
    schedule(task_cx_ptr);
}
