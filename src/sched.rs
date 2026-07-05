//! 轮转调度器。时钟中断驱动，每个时间片切换一次任务。

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::boxed::Box;
use crate::task::{Task, TaskContext, TaskState};
use crate::trap::TrapContext;

const MAX_TASKS: usize = 32;
const TIME_SLICE: u64 = 2; // 每 2 个 tick（20ms）切换一次

struct Scheduler {
    tasks: [Option<Box<Task>>; MAX_TASKS],
    current: usize, // 当前运行任务索引；MAX_TASKS 表示无（idle）
    next_id: usize,
}

impl Scheduler {
    const fn new() -> Self {
        const NONE: Option<Box<Task>> = None;
        Self {
            tasks: [NONE; MAX_TASKS],
            current: MAX_TASKS,
            next_id: 0,
        }
    }

    /// 找下一个 Ready 任务的索引（从 current+1 起轮转）
    fn pick_next(&self) -> Option<usize> {
        if self.current == MAX_TASKS {
            // 从 0 找
            for i in 0..MAX_TASKS {
                if let Some(t) = &self.tasks[i] {
                    if t.state == TaskState::Ready {
                        return Some(i);
                    }
                }
            }
            return None;
        }
        for k in 1..=MAX_TASKS {
            let i = (self.current + k) % MAX_TASKS;
            if let Some(t) = &self.tasks[i] {
                if t.state == TaskState::Ready {
                    return Some(i);
                }
            }
        }
        None
    }
}

// 自旋锁
use core::cell::UnsafeCell;
struct Spinlock<T> {
    locked: AtomicU64,
    data: UnsafeCell<T>,
}
unsafe impl<T: Send> Sync for Spinlock<T> {}
impl<T> Spinlock<T> {
    const fn new(t: T) -> Self {
        Self {
            locked: AtomicU64::new(0),
            data: UnsafeCell::new(t),
        }
    }
    fn lock(&self) -> &mut T {
        while self
            .locked
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) != 0 {
                core::hint::spin_loop();
            }
        }
        unsafe { &mut *self.data.get() }
    }
    unsafe fn unlock(&self) {
        self.locked.store(0, Ordering::Release);
    }
}

static SCHED: Spinlock<Scheduler> = Spinlock::new(Scheduler::new());
static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
static CURRENT_CTX_PTR: AtomicUsize = AtomicUsize::new(0); // 当前任务 task_ctx 指针

/// 注册一个内核任务
pub fn spawn(entry: usize, name: &'static str) -> usize {
    let s = SCHED.lock();
    let id = s.next_id;
    s.next_id += 1;
    // 找空槽
    let mut slot = None;
    for i in 0..MAX_TASKS {
        if s.tasks[i].is_none() {
            slot = Some(i);
            break;
        }
    }
    let i = slot.expect("too many tasks");
    let task = Task::new_kernel(id, entry, name);
    s.tasks[i] = Some(task);
    unsafe { SCHED.unlock(); }
    crate::println!("[sched] spawned task #{} '{}' @ slot {}", id, name, i);
    id
}

/// 时钟中断时调用
pub fn on_tick() {
    let t = TICK_COUNT.fetch_add(1, Ordering::SeqCst);
    if t % TIME_SLICE != 0 {
        return;
    }
    schedule();
}

fn schedule() {
    let s = SCHED.lock();
    let cur = s.current;
    // 当前任务若 Running 则置回 Ready
    if cur != MAX_TASKS {
        if let Some(t) = &s.tasks[cur] {
            if t.state == TaskState::Running {
                // 借用冲突：用 unsafe 设置
            }
        }
    }
    let next = match s.pick_next() {
        Some(n) => n,
        None => {
            unsafe { SCHED.unlock(); }
            return; // 无其他任务，继续当前
        }
    };
    if next == cur {
        unsafe { SCHED.unlock(); }
        return;
    }
    // 取得两个 task_ctx 指针
    let cur_ctx_ptr = if cur != MAX_TASKS {
        let t = s.tasks[cur].as_ref().unwrap();
        // 标记 Ready
        let tptr = t.as_ref() as *const Task as *mut Task;
        unsafe { (*tptr).state = TaskState::Ready; }
        &unsafe { &*tptr }.task_ctx as *const TaskContext as *mut TaskContext
    } else {
        // 从 idle 启动：用引导栈上的 dummy 上下文（仅一次性）
        // 用静态 dummy
        static mut DUMMY: TaskContext = TaskContext::zero();
        unsafe { &mut DUMMY as *mut TaskContext }
    };
    let next_task_ptr = s.tasks[next].as_ref().unwrap().as_ref() as *const Task as *mut Task;
    let next_ctx_ptr = unsafe { &mut (*next_task_ptr).task_ctx as *mut TaskContext };
    unsafe { (*next_task_ptr).state = TaskState::Running; }
    s.current = next;
    CURRENT_CTX_PTR.store(next_ctx_ptr as usize, Ordering::SeqCst);
    unsafe { SCHED.unlock(); }

    // 执行切换
    unsafe {
        crate::task::switch_to(cur_ctx_ptr, next_ctx_ptr);
    }
}

/// 首次进入调度：从 idle/主上下文切换到第一个任务
pub fn run_first_task() -> ! {
    let s = SCHED.lock();
    let next = s.pick_next().expect("no task to run");
    let next_task_ptr = s.tasks[next].as_ref().unwrap().as_ref() as *const Task as *mut Task;
    let next_ctx_ptr = unsafe { &mut (*next_task_ptr).task_ctx as *mut TaskContext };
    unsafe { (*next_task_ptr).state = TaskState::Running; }
    s.current = next;
    CURRENT_CTX_PTR.store(next_ctx_ptr as usize, Ordering::SeqCst);
    unsafe { SCHED.unlock(); }

    static mut DUMMY: TaskContext = TaskContext::zero();
    let dummy = unsafe { &mut DUMMY as *mut TaskContext };
    crate::println!("[sched] starting first task (slot {})", next);
    unsafe {
        crate::task::switch_to(dummy, next_ctx_ptr);
    }
    // 不应返回
    crate::println!("[sched] ERROR: returned from first switch");
    crate::sbi::shutdown();
}

/// 当前任务退出
pub fn exit_current(code: i32) -> ! {
    let s = SCHED.lock();
    let cur = s.current;
    if cur != MAX_TASKS {
        let t = s.tasks[cur].as_ref().unwrap();
        let tptr = t.as_ref() as *const Task as *mut Task;
        unsafe { (*tptr).state = TaskState::Exited; }
        crate::println!("[sched] task '{}' exited (code={})", (*tptr).name, code);
    }
    unsafe { SCHED.unlock(); }
    // 切换走，不再返回
    schedule();
    crate::sbi::shutdown();
}

/// 供 syscall 路径占位使用
pub fn current_trap_ctx() -> *mut TrapContext {
    // 当前任务上下文指针的 sp 字段指向其 TrapContext
    let p = CURRENT_CTX_PTR.load(Ordering::SeqCst) as *const TaskContext;
    if p.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { (*p).sp as *mut TrapContext }
    }
}
