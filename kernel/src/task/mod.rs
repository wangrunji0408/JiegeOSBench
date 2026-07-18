pub mod context;
mod manager;
mod pid;
mod processor;
mod task;

use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use context::TaskContext;
use core::arch::global_asm;
use spin::{Mutex, Once};
use task::TaskControlBlock;

global_asm!(include_str!("switch.S"));

pub use manager::add_task;
pub use processor::{
    current_task, current_trap_cx, current_user_token, run_tasks, schedule,
    suspend_current_and_run_next, take_current_task,
};
pub use task::TaskStatus;

static INITPROC: Once<Arc<TaskControlBlock>> = Once::new();

/// Global pid -> task lookup, used by `kill()` to reach a task that isn't
/// the caller or one of its direct children. Entries for exited tasks are
/// harmless (`Weak::upgrade` just returns `None`) and are pruned lazily on
/// the next lookup that walks past them; this workload only ever has a
/// handful of processes, so the unbounded-until-then growth doesn't matter.
static PID_TABLE: Mutex<BTreeMap<usize, Weak<TaskControlBlock>>> = Mutex::new(BTreeMap::new());

fn register_task(task: &Arc<TaskControlBlock>) {
    PID_TABLE.lock().insert(task.pid(), Arc::downgrade(task));
}

pub fn find_task_by_pid(pid: usize) -> Option<Arc<TaskControlBlock>> {
    PID_TABLE.lock().get(&pid).and_then(Weak::upgrade)
}

pub fn add_initproc(elf_data: &[u8], args: &[alloc::string::String], envs: &[alloc::string::String]) {
    let tcb = TaskControlBlock::new_initproc(elf_data, args, envs);
    register_task(&tcb);
    INITPROC.call_once(|| tcb.clone());
    add_task(tcb);
}

pub fn exit_current_and_run_next(exit_code: i32) -> ! {
    let task = take_current_task().unwrap();
    let mut inner = task.inner_lock();
    inner.task_status = TaskStatus::Zombie;
    inner.exit_code = exit_code;

    if let Some(initproc) = INITPROC.get() {
        if !Arc::ptr_eq(&task, initproc) {
            let mut init_inner = initproc.inner_lock();
            for child in inner.children.iter() {
                child.inner_lock().parent = Some(Arc::downgrade(initproc));
                init_inner.children.push(child.clone());
            }
        }
    }
    inner.children.clear();
    let parent = inner.parent.clone();
    drop(inner);
    if let Some(parent) = parent.and_then(|p| p.upgrade()) {
        parent.inner_lock().signals.raise(crate::signal::SIGCHLD);
    }
    drop(task);

    let mut unused = TaskContext::zero_init();
    schedule(&mut unused as *mut _);
    unreachable!("exited task should never be scheduled again");
}

/// Read-only convenience used by syscalls that need the running task's PID.
pub fn current_pid() -> usize {
    current_task().unwrap().pid()
}
