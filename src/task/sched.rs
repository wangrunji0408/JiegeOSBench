//! The scheduler.
//!
//! A single-hart round-robin scheduler. The idle context is the boot stack; each
//! reschedule switches from the current task to the idle loop, which picks the
//! next runnable task and switches into it. Tasks that block simply yield with
//! their state set to `Blocked` and are woken by whoever satisfies them.

use super::task::{Task, TaskState};
use crate::fs::InodeRef;
use crate::trap::{TaskContext, TrapContext};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

/// The run queue.
static READY: Mutex<VecDeque<Arc<Task>>> = Mutex::new(VecDeque::new());
/// Every live task, for signal delivery and `wait`.
static ALL_TASKS: Mutex<Vec<Arc<Task>>> = Mutex::new(Vec::new());

/// The currently running task. `None` while the idle loop runs.
static mut CURRENT: Option<Arc<Task>> = None;
/// The idle (scheduler) context, saved when we switch into a task.
static mut IDLE_CONTEXT: TaskContext = TaskContext {
    ra: 0,
    sp: 0,
    s: [0; 12],
};

static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
static CONTEXT_SWITCHES: AtomicUsize = AtomicUsize::new(0);
/// Ticks the current task has consumed of its quantum.
static QUANTUM: AtomicUsize = AtomicUsize::new(0);
/// Timeslice length in timer ticks (10 ms each).
const TIMESLICE: usize = 3;

pub fn context_switches() -> usize {
    CONTEXT_SWITCHES.load(Ordering::Relaxed)
}

/// Add a task to the run queue.
pub fn spawn(task: Arc<Task>) {
    ALL_TASKS.lock().push(task.clone());
    task.set_state(TaskState::Runnable);
    READY.lock().push_back(task);
}

/// Put a task back on the run queue (used by `wake`).
pub fn enqueue(task: Arc<Task>) {
    let mut state = task.state.lock();
    if *state == TaskState::Zombie {
        return;
    }
    if *state == TaskState::Runnable {
        // Avoid double-queueing a task that is already runnable.
        let queued = READY.lock().iter().any(|t| t.tid == task.tid);
        if queued {
            return;
        }
    }
    *state = TaskState::Runnable;
    drop(state);
    READY.lock().push_back(task);
}

/// Is there a current task? False inside the idle loop.
pub fn has_current() -> bool {
    unsafe {
        #[allow(static_mut_refs)]
        CURRENT.is_some()
    }
}

/// The currently running task.
pub fn current() -> Arc<Task> {
    unsafe {
        #[allow(static_mut_refs)]
        CURRENT.as_ref().expect("no current task").clone()
    }
}

/// The current task without cloning the `Arc` (hot path in the syscall layer).
pub fn with_current<T>(f: impl FnOnce(&Arc<Task>) -> T) -> T {
    unsafe {
        #[allow(static_mut_refs)]
        f(CURRENT.as_ref().expect("no current task"))
    }
}

pub fn current_trap_context() -> *mut TrapContext {
    unsafe {
        #[allow(static_mut_refs)]
        CURRENT
            .as_ref()
            .expect("no current task")
            .trap_context_ptr()
    }
}

pub fn current_cwd() -> InodeRef {
    with_current(|t| t.cwd.lock().clone())
}

pub fn current_pid() -> usize {
    with_current(|t| t.pid())
}

pub fn current_tid() -> usize {
    with_current(|t| t.tid)
}

/// Look up a task by tid.
pub fn find_task(tid: usize) -> Option<Arc<Task>> {
    ALL_TASKS.lock().iter().find(|t| t.tid == tid).cloned()
}

/// Look up the group leader of a process by pid.
pub fn find_process(pid: usize) -> Option<Arc<Task>> {
    ALL_TASKS
        .lock()
        .iter()
        .find(|t| t.pid() == pid && t.is_group_leader())
        .cloned()
}

/// Every task in a process group.
pub fn tasks_in_pgroup(pgid: usize) -> Vec<Arc<Task>> {
    ALL_TASKS
        .lock()
        .iter()
        .filter(|t| t.pgid() == pgid && t.is_group_leader())
        .cloned()
        .collect()
}

/// Every live task.
pub fn all_tasks() -> Vec<Arc<Task>> {
    ALL_TASKS.lock().clone()
}

/// Request a reschedule at the next opportunity.
pub fn request_reschedule() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
}

/// Called from the timer tick: consume the quantum.
pub fn on_tick() {
    if !has_current() {
        return;
    }
    let used = QUANTUM.fetch_add(1, Ordering::Relaxed) + 1;
    if used >= TIMESLICE {
        request_reschedule();
    }
    // Charge the tick to the current process.
    with_current(|t| {
        t.group.utime.fetch_add(1, Ordering::Relaxed);
    });
}

/// If a reschedule was requested, yield now.
pub fn check_reschedule() {
    if NEED_RESCHED.swap(false, Ordering::Relaxed) {
        QUANTUM.store(0, Ordering::Relaxed);
        // Only bother if someone else is waiting to run.
        if !READY.lock().is_empty() {
            yield_now();
        }
    }
}

/// Give up the CPU, staying runnable.
pub fn yield_now() {
    if !has_current() {
        return;
    }
    switch_to_idle(TaskState::Runnable);
}

/// Block the current task until someone wakes it.
pub fn block_current() {
    if !has_current() {
        return;
    }
    switch_to_idle(TaskState::Blocked);
}

/// Save the current task with `state` and return to the idle loop.
fn switch_to_idle(state: TaskState) {
    let was_enabled = crate::trap::disable_interrupts();
    let task = current();
    task.set_state(state);
    if state == TaskState::Runnable {
        READY.lock().push_back(task.clone());
    }
    let task_cx_ptr = {
        let mut cx = task.task_cx.lock();
        &mut *cx as *mut TaskContext
    };
    unsafe {
        #[allow(static_mut_refs)]
        let idle = &raw const IDLE_CONTEXT;
        CURRENT = None;
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        crate::trap::__switch(task_cx_ptr, idle);
    }
    // We are back: this task was rescheduled.
    crate::trap::restore_interrupts(was_enabled);
}

/// The idle loop: pick a task and run it, forever.
pub fn run() -> ! {
    loop {
        let next = READY.lock().pop_front();
        let Some(task) = next else {
            // Nothing to run. Reap dead tasks and wait for an interrupt.
            reap_zombies();
            if ALL_TASKS.lock().iter().all(|t| t.is_zombie()) && !ALL_TASKS.lock().is_empty() {
                crate::info!("all tasks exited; shutting down");
                crate::sbi::shutdown(false);
            }
            crate::trap::enable_interrupts();
            crate::arch::wfi();
            // Poll the network device: with no runnable task, nothing else will.
            crate::net::poll();
            continue;
        };

        // Skip tasks that died or blocked since being queued.
        {
            let state = *task.state.lock();
            if state == TaskState::Zombie || state == TaskState::Blocked {
                continue;
            }
        }

        crate::trap::disable_interrupts();
        task.set_state(TaskState::Runnable);
        let satp = task.aspace.lock().satp();
        crate::mm::page_table::activate(satp);

        let task_cx_ptr = {
            let cx = task.task_cx.lock();
            &*cx as *const TaskContext
        };
        unsafe {
            CURRENT = Some(task);
            #[allow(static_mut_refs)]
            let idle = &raw mut IDLE_CONTEXT;
            CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
            crate::trap::__switch(idle, task_cx_ptr);
        }
        // The task yielded back to us.
        unsafe {
            CURRENT = None;
        }
        // Service the network stack between tasks so packets flow even when a
        // busy task never blocks.
        crate::net::poll();
    }
}

/// Remove exited tasks whose parents have reaped them.
fn reap_zombies() {
    let mut all = ALL_TASKS.lock();
    all.retain(|t| {
        // A zombie with only our reference left (the parent's `children` list
        // dropped it after `wait4`) can go.
        !(t.is_zombie() && Arc::strong_count(t) <= 1)
    });
}

/// Terminate the current task. Never returns.
pub fn exit_current(exit_code: i32) -> ! {
    let task = current();
    do_exit(&task, exit_code);

    // Switch away for the last time; the idle loop will not pick us up again.
    crate::trap::disable_interrupts();
    let task_cx_ptr = {
        let mut cx = task.task_cx.lock();
        &mut *cx as *mut TaskContext
    };
    unsafe {
        #[allow(static_mut_refs)]
        let idle = &raw const IDLE_CONTEXT;
        CURRENT = None;
        crate::trap::__switch(task_cx_ptr, idle);
    }
    unreachable!("exited task was rescheduled");
}

/// Mark a task dead and clean up its resources.
fn do_exit(task: &Arc<Task>, exit_code: i32) {
    task.exit_code.store(exit_code, Ordering::Relaxed);
    task.set_state(TaskState::Zombie);

    // CLONE_CHILD_CLEARTID: zero the tid and wake futex waiters, which is how
    // pthread_join learns the thread is gone.
    let clear_tid = task.clear_child_tid.swap(0, Ordering::Relaxed);
    if clear_tid != 0 {
        let _ = crate::mm::uaccess::write(clear_tid, 0u32);
        super::futex::wake(clear_tid, i32::MAX as usize);
    }

    // If this is the last thread in the group, finish off the process.
    let remaining = {
        let threads = task.group.threads.lock();
        threads
            .iter()
            .filter_map(|w| w.upgrade())
            .filter(|t| !t.is_zombie())
            .count()
    };

    if remaining == 0 {
        task.group.exit_code.store(exit_code, Ordering::Relaxed);
        // Release the address space now rather than waiting to be reaped, so a
        // long-lived parent doesn't pin the child's memory.
        task.aspace.lock().clear_user();
        // Close the fd table so listening sockets are released.
        task.files.lock().close_range(0, u32::MAX);

        // Reparent our children to init (pid 1).
        let orphans: Vec<Arc<Task>> = task.group.children.lock().drain(..).collect();
        if !orphans.is_empty() {
            if let Some(init) = find_process(1) {
                for child in orphans {
                    child.group.ppid.store(1, Ordering::Relaxed);
                    init.group.children.lock().push(child);
                }
            }
        }

        // Notify the parent with SIGCHLD and wake it if it is in `wait4`.
        let ppid = task.ppid();
        if ppid != 0 {
            if let Some(parent) = find_process(ppid) {
                crate::signal::send_to_process(&parent, crate::signal::SIGCHLD);
                // Wake every thread of the parent, since any could be waiting.
                for weak in parent.group.threads.lock().iter() {
                    if let Some(t) = weak.upgrade() {
                        if t.get_state() == TaskState::Blocked {
                            enqueue(t);
                        }
                    }
                }
            }
        }
    }
}

/// Exit every thread in the current thread group.
pub fn exit_group(exit_code: i32) -> ! {
    let task = current();
    task.group.group_exiting.store(true, Ordering::Relaxed);
    task.group.exit_code.store(exit_code, Ordering::Relaxed);

    // Kill the siblings. They will notice `group_exiting` and exit; for those
    // that are blocked we mark them zombie directly, since they hold no locks
    // we need.
    let siblings: Vec<Arc<Task>> = task
        .group
        .threads
        .lock()
        .iter()
        .filter_map(|w| w.upgrade())
        .filter(|t| t.tid != task.tid && !t.is_zombie())
        .collect();
    for sibling in siblings {
        sibling.exit_code.store(exit_code, Ordering::Relaxed);
        sibling.set_state(TaskState::Zombie);
    }

    exit_current(exit_code)
}

/// True if the current task has a signal that should interrupt a blocking call.
pub fn has_pending_signal() -> bool {
    if !has_current() {
        return false;
    }
    with_current(|task| {
        if task.group.group_exiting.load(Ordering::Relaxed) {
            return true;
        }
        let signals = task.signals.lock();
        signals.has_deliverable()
    })
}

/// Block until `cond` returns true, or a signal arrives.
///
/// Returns `true` if the condition was satisfied, `false` if interrupted.
pub fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    loop {
        if cond() {
            return true;
        }
        if has_pending_signal() {
            return false;
        }
        yield_now();
    }
}

/// Block until `cond` is true or the deadline (in ms since boot) passes.
pub fn wait_until_timeout(mut cond: impl FnMut() -> bool, deadline_ms: Option<u64>) -> bool {
    loop {
        if cond() {
            return true;
        }
        if let Some(deadline) = deadline_ms {
            if crate::time::monotonic_ms() >= deadline {
                return false;
            }
        }
        if has_pending_signal() {
            return false;
        }
        yield_now();
    }
}

/// Make a task current during boot, before the scheduler runs.
///
/// `loader::exec` and the uaccess helpers need a current task; at boot there
/// isn't one yet, so we install it temporarily. The task's page table must be
/// active for the loader's writes to the new stack to land.
pub fn set_current_for_init(task: Arc<Task>) {
    let satp = task.aspace.lock().satp();
    crate::mm::page_table::activate(satp);
    unsafe {
        CURRENT = Some(task);
    }
}

/// Undo [`set_current_for_init`], restoring the kernel page table.
pub fn clear_current_for_init() {
    unsafe {
        CURRENT = None;
    }
    let satp = crate::mm::vma::kernel_page_table().satp();
    crate::mm::page_table::activate(satp);
}

/// Deliver pending signals — re-exported so the trap handler can call
/// `task::handle_pending_signals`.
pub fn handle_pending_signals(cx: &mut TrapContext) {
    crate::signal::handle_pending(cx);
}

/// Raise a signal on the current task and terminate if it is fatal.
pub fn force_signal(sig: usize) {
    if !has_current() {
        return;
    }
    let task = current();
    // Clear any handler-blocking so a fault-generated signal always lands: Linux
    // forces the default action when a synchronous fault signal is blocked.
    {
        let mut signals = task.signals.lock();
        signals.mask.remove(sig);
    }
    crate::signal::send_to_task(&task, sig);
}

/// Send a signal to the current task (used by the pipe/socket EPIPE paths).
pub fn send_signal_to_self(sig: usize) {
    if !has_current() {
        return;
    }
    let task = current();
    crate::signal::send_to_task(&task, sig);
}
