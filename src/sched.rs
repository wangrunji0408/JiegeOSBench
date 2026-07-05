//! 进程调度器：轮转调度用户进程，无就绪进程时切回 idle。

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use alloc::boxed::Box;
use crate::task::{TaskContext, TaskState};
use crate::process::Process;
use crate::mm::page_table::{kernel_pt, set_satp};

const MAX_PROCS: usize = 32;
const TIME_SLICE: u64 = 3; // 每 3 tick 切换

struct Scheduler {
    procs: [Option<Box<Process>>; MAX_PROCS],
    current: usize, // 当前进程索引；MAX_PROCS 表示 idle
}

impl Scheduler {
    const fn new() -> Self {
        const NONE: Option<Box<Process>> = None;
        Self {
            procs: [NONE; MAX_PROCS],
            current: MAX_PROCS,
        }
    }

    fn pick_next(&self) -> Option<usize> {
        if self.current == MAX_PROCS {
            for i in 0..MAX_PROCS {
                if let Some(p) = &self.procs[i] {
                    if p.state == TaskState::Ready {
                        return Some(i);
                    }
                }
            }
            return None;
        }
        for k in 1..=MAX_PROCS {
            let i = (self.current + k) % MAX_PROCS;
            if let Some(p) = &self.procs[i] {
                if p.state == TaskState::Ready {
                    return Some(i);
                }
            }
        }
        None
    }
}

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
static mut IDLE_CTX: TaskContext = TaskContext::zero();

/// 注册一个进程
pub fn spawn(elf: &[u8], name: &'static str) -> usize {
    let s = SCHED.lock();
    let pid = next_pid();
    let proc = match Process::from_elf(elf, pid, name) {
        Some(p) => p,
        None => {
            unsafe { SCHED.unlock(); }
            crate::println!("[sched] failed to load process '{}'", name);
            return usize::MAX;
        }
    };
    let mut slot = None;
    for i in 0..MAX_PROCS {
        if s.procs[i].is_none() {
            slot = Some(i);
            break;
        }
    }
    let i = slot.expect("too many procs");
    s.procs[i] = Some(proc);
    unsafe { SCHED.unlock(); }
    crate::println!("[sched] spawned pid={} '{}' @ slot {}", pid, name, i);
    pid
}

fn next_pid() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

pub fn on_tick() {
    let t = TICK_COUNT.fetch_add(1, Ordering::SeqCst);
    if t % TIME_SLICE != 0 {
        return;
    }
    schedule();
}

fn set_satp_for(proc_root_pa: Option<usize>) {
    let root = proc_root_pa.unwrap_or_else(|| kernel_pt().root_pa);
    unsafe { set_satp((8usize << 60) | (root >> 12)); }
}

fn schedule() {
    let s = SCHED.lock();
    let cur = s.current;
    // 当前进程置回 Ready
    if cur != MAX_PROCS {
        if let Some(p) = s.procs[cur].as_ref() {
            let pp = p.as_ref() as *const Process as *mut Process;
            if unsafe { (*pp).state } == TaskState::Running {
                unsafe { (*pp).state = TaskState::Ready; }
            }
        }
    }
    let next = match s.pick_next() {
        Some(n) => n,
        None => {
            // 无就绪进程：切回 idle（若已在 idle 则直接返回）
            if cur == MAX_PROCS {
                unsafe { SCHED.unlock(); }
                return;
            }
            // 当前是进程，切到 idle
            let cur_ctx_ptr = {
                let p = s.procs[cur].as_ref().unwrap();
                &(p.as_ref().task_ctx) as *const TaskContext as *mut TaskContext
            };
            s.current = MAX_PROCS;
            set_satp_for(None);
            let idle_ptr = unsafe { &mut IDLE_CTX as *mut TaskContext };
            unsafe { SCHED.unlock(); }
            unsafe { crate::task::switch_to(cur_ctx_ptr, idle_ptr); }
            return;
        }
    };
    if next == cur {
        unsafe { SCHED.unlock(); }
        return;
    }

    let cur_ctx_ptr: *mut TaskContext = if cur != MAX_PROCS {
        let p = s.procs[cur].as_ref().unwrap();
        &(p.as_ref().task_ctx) as *const TaskContext as *mut TaskContext
    } else {
        unsafe { &mut IDLE_CTX as *mut TaskContext }
    };
    let next_proc_ptr = s.procs[next].as_ref().unwrap().as_ref() as *const Process as *mut Process;
    let next_ctx_ptr = unsafe { &mut (*next_proc_ptr).task_ctx as *mut TaskContext };
    unsafe { (*next_proc_ptr).state = TaskState::Running; }
    let next_root = unsafe { (*next_proc_ptr).root_pa };
    s.current = next;
    set_satp_for(Some(next_root));
    unsafe { SCHED.unlock(); }
    unsafe { crate::task::switch_to(cur_ctx_ptr, next_ctx_ptr); }
}

/// 首次进入调度：切到第一个就绪进程
pub fn run_first_task() -> ! {
    let s = SCHED.lock();
    let next = s.pick_next().expect("no process to run");
    let nptr = s.procs[next].as_ref().unwrap().as_ref() as *const Process as *mut Process;
    let next_ctx_ptr = unsafe { &mut (*nptr).task_ctx as *mut TaskContext };
    unsafe { (*nptr).state = TaskState::Running; }
    let next_root = unsafe { (*nptr).root_pa };
    s.current = next;
    set_satp_for(Some(next_root));
    unsafe { SCHED.unlock(); }
    crate::println!("[sched] starting first process (pid at slot {})", next);
    let dummy = unsafe { &mut IDLE_CTX as *mut TaskContext };
    unsafe { crate::task::switch_to(dummy, next_ctx_ptr); }
    // 切回 idle 时会回到这里
    crate::println!("[sched] idle context resumed");
    idle_loop();
}

fn idle_loop() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi"); }
    }
}

/// 当前进程退出
pub fn exit_current(code: i32) -> ! {
    let s = SCHED.lock();
    let cur = s.current;
    if cur != MAX_PROCS {
        let p = s.procs[cur].as_ref().unwrap();
        let pp = p.as_ref() as *const Process as *mut Process;
        unsafe { (*pp).state = TaskState::Exited; }
        crate::println!("[sched] pid {} '{}' exited (code={})", unsafe {(*pp).pid}, unsafe {(*pp).name}, code);
    }
    unsafe { SCHED.unlock(); }
    schedule();
    // 若无其他进程，回到 idle
    idle_loop();
}
