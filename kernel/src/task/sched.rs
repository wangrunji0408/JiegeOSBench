//! Round-robin scheduler. Single core; the kernel is non-preemptive.
use alloc::collections::VecDeque;
use alloc::sync::Arc;

use super::{Task, TaskState, CURRENT};
use crate::sync::SpinLock;
use crate::trap::{__switch, csr, Context};

static RUNQUEUE: SpinLock<VecDeque<Arc<Task>>> = SpinLock::new(VecDeque::new());
static mut IDLE_CTX: Context = Context::zero();
/// Physical root of the currently active page table (0 = kernel/bare).
static mut ACTIVE_SATP: usize = 0;

pub fn make_runnable(task: &Arc<Task>) {
    let mut inner = task.inner.lock();
    if inner.state == TaskState::Blocked || inner.state == TaskState::Running {
        inner.state = TaskState::Runnable;
        drop(inner);
        RUNQUEUE.lock().push_back(task.clone());
    } else if inner.state == TaskState::Runnable {
        // Freshly created task not yet queued.
        drop(inner);
        let mut rq = RUNQUEUE.lock();
        if !rq.iter().any(|t| Arc::ptr_eq(t, task)) {
            rq.push_back(task.clone());
        }
    }
}

/// Activate the address space of `task` if different from the current one.
pub fn activate_mm(task: &Task) {
    let satp = task.mm().lock().satp();
    activate_satp(satp);
}

pub fn activate_satp(satp: usize) {
    unsafe {
        if ACTIVE_SATP != satp {
            csr::write_satp(satp);
            core::arch::asm!("sfence.vma");
            ACTIVE_SATP = satp;
        }
    }
}

/// Force a TLB flush + reload of the current task's page table (after execve).
pub fn reload_mm(task: &Task) {
    let satp = task.mm().lock().satp();
    unsafe {
        csr::write_satp(satp);
        core::arch::asm!("sfence.vma");
        ACTIVE_SATP = satp;
    }
}

fn pick_next() -> Option<Arc<Task>> {
    RUNQUEUE.lock().pop_front()
}

/// Switch away from the current task. The caller must have already set the
/// current task's state (Runnable + queued, Blocked, or Zombie).
pub fn schedule() {
    let cur = CURRENT.get().clone();
    let next = pick_next();
    match next {
        Some(next) => {
            if let Some(c) = &cur {
                if Arc::ptr_eq(c, &next) {
                    next.inner.lock().state = TaskState::Running;
                    return;
                }
            }
            switch_to(cur, next);
        }
        None => {
            // nothing runnable: go to idle context
            let Some(c) = cur else { return };
            *CURRENT.get() = None;
            unsafe { __switch(c.ctx_ptr(), core::ptr::addr_of!(IDLE_CTX)) };
            // resumed
        }
    }
}

fn switch_to(cur: Option<Arc<Task>>, next: Arc<Task>) {
    next.inner.lock().state = TaskState::Running;
    activate_mm(&next);
    let next_ctx = next.ctx_ptr();
    *CURRENT.get() = Some(next);
    match cur {
        Some(c) => unsafe { __switch(c.ctx_ptr(), next_ctx) },
        None => unsafe { __switch(core::ptr::addr_of_mut!(IDLE_CTX), next_ctx) },
    }
}

/// Give up the CPU but stay runnable.
pub fn yield_now() {
    let cur = super::current();
    if RUNQUEUE.lock().is_empty() {
        return;
    }
    {
        let mut inner = cur.inner.lock();
        inner.state = TaskState::Runnable;
    }
    RUNQUEUE.lock().push_back(cur.clone());
    schedule();
}

/// Block the current task until woken by `make_runnable`.
pub fn block_current() {
    let cur = super::current();
    cur.inner.lock().state = TaskState::Blocked;
    schedule();
}

/// The idle loop, run on the boot stack once the first task exists.
pub fn idle_loop() -> ! {
    loop {
        csr::disable_interrupts();
        if let Some(next) = pick_next() {
            switch_to(None, next);
            // back in idle
            continue;
        }
        // Wait for an interrupt with interrupts disabled, then briefly enable
        // them to take it (no lost-wakeup window).
        csr::wfi();
        csr::enable_interrupts();
        csr::disable_interrupts();
    }
}

pub fn runqueue_len() -> usize {
    RUNQUEUE.lock().len()
}
