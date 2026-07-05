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
static CURRENT_PROC: AtomicUsize = AtomicUsize::new(0); // 当前进程裸指针；0=idle

/// 当前进程（syscall 路径无锁获取）
pub fn current_process() -> Option<&'static mut Process> {
    let p = CURRENT_PROC.load(Ordering::SeqCst);
    if p == 0 {
        None
    } else {
        unsafe { Some(&mut *(p as *mut Process)) }
    }
}

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

/// clone：创建子进程（共享父进程地址空间，复制 trap context）
/// flags=SIGCHLD(17)，子进程返回 0，父进程返回子 pid
pub fn clone_child(flags: usize, stack: usize, ptid: usize, _ctid: usize) -> isize {
    let s = SCHED.lock();
    let cur = s.current;
    if cur == MAX_PROCS {
        unsafe { SCHED.unlock(); }
        return -3;
    }
    let parent_pid;
    let parent_root;
    let parent_brk;
    let parent_brk_start;
    let parent_next_mmap;
    let parent_tid_addr;
    let parent_kstack_top;
    let parent_trap_ctx_ptr;
    {
        let p = s.procs[cur].as_ref().unwrap();
        let pp = p.as_ref() as *const Process as *mut Process;
        parent_pid = unsafe { (*pp).pid };
        parent_root = unsafe { (*pp).root_pa };
        parent_brk = unsafe { (*pp).brk };
        parent_brk_start = unsafe { (*pp).brk_start };
        parent_next_mmap = unsafe { (*pp).next_mmap };
        parent_tid_addr = unsafe { (*pp).tid_address };
        parent_kstack_top = unsafe { (*pp).kstack_top };
        parent_trap_ctx_ptr = unsafe { (*pp).trap_ctx_ptr };
    }
    let _ = flags;
    // 分配新内核栈
    use crate::mm::PAGE_SIZE;
    let pages = crate::task::KSTACK_SIZE / PAGE_SIZE;
    let mut kstack_base = 0usize;
    for i in 0..pages {
        let pa = crate::mm::frame::FRAME_ALLOCATOR.alloc_zeroed().unwrap();
        if i == 0 { kstack_base = pa; }
    }
    let new_kstack_top = kstack_base + crate::task::KSTACK_SIZE;
    // 复制 trap context
    let ctx_ptr = (new_kstack_top - core::mem::size_of::<crate::trap::TrapContext>())
        as *mut crate::trap::TrapContext;
    unsafe {
        let parent_ctx = &*(parent_trap_ctx_ptr as *const crate::trap::TrapContext);
        let child_ctx = &mut *ctx_ptr;
        *child_ctx = parent_ctx.clone();
        // 子进程返回值 a0=0
        child_ctx.x[10] = 0;
        // 如果指定了新栈，用新栈
        if stack != 0 {
            child_ctx.x[2] = stack;
        }
    }
    let child_pid = next_pid();
    let child = Process {
        pid: child_pid,
        task_ctx: crate::task::TaskContext {
            ra: crate::task::__restore as *const () as usize,
            sp: ctx_ptr as usize,
            s: [0; 12],
        },
        kstack_top: new_kstack_top,
        trap_ctx_ptr: ctx_ptr as usize,
        root_pa: parent_root, // 共享地址空间
        state: crate::task::TaskState::Ready,
        name: "nginx-worker",
        brk: parent_brk,
        brk_start: parent_brk_start,
        next_mmap: parent_next_mmap,
        fd_table: crate::vfs::FdTable::new(),
        sock_table: crate::process::SockTable::new(),
        tid_address: if ptid != 0 { ptid } else { parent_tid_addr },
        set_child_tid: ptid,
    };
    // 找空槽
    let mut slot = None;
    for i in 0..MAX_PROCS {
        if s.procs[i].is_none() {
            slot = Some(i);
            break;
        }
    }
    let i = match slot {
        Some(i) => i,
        None => { unsafe { SCHED.unlock(); } return -12; }
    };
    s.procs[i] = Some(unsafe { Box::from_raw(Box::into_raw(Box::new(child))) });
    unsafe { SCHED.unlock(); }
    // 父进程在 ptid 写子 pid
    if ptid != 0 {
        unsafe { core::ptr::write_volatile(ptid as *mut i32, child_pid as i32); }
    }
    crate::println!("[sched] cloned pid {} -> {}", parent_pid, child_pid);
    child_pid as isize
}

extern "C" {
    fn __restore();
}

fn next_pid() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// 当前进程的 pid（idle 返回 0）
pub fn current_pid() -> usize {
    let s = SCHED.lock();
    let cur = s.current;
    let pid = if cur != MAX_PROCS {
        s.procs[cur].as_ref().map(|p| p.as_ref().pid).unwrap_or(0)
    } else {
        0
    };
    unsafe { SCHED.unlock(); }
    pid
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
            CURRENT_PROC.store(0, Ordering::SeqCst);
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
    CURRENT_PROC.store(next_proc_ptr as usize, Ordering::SeqCst);
    set_satp_for(Some(next_root));
    unsafe { SCHED.unlock(); }
    unsafe { crate::task::switch_to(cur_ctx_ptr, next_ctx_ptr); }
}

/// 首次进入调度：切到第一个就绪进程；无进程则直接进 idle
pub fn run_first_task() -> ! {
    // 关 SIE 防止时钟中断在首次切换前打断（避免竞态）
    unsafe { core::arch::asm!("csrci sstatus, 0x2"); }
    crate::println!("[sched] run_first_task entered");
    let next = {
        let s = SCHED.lock();
        let n = s.pick_next();
        unsafe { SCHED.unlock(); }
        n
    };
    crate::println!("[sched] pick_next = {:?}", next);
    match next {
        Some(next) => {
            let s = SCHED.lock();
            let nptr = s.procs[next].as_ref().unwrap().as_ref() as *const Process as *mut Process;
            let next_ctx_ptr = unsafe { &mut (*nptr).task_ctx as *mut TaskContext };
            unsafe { (*nptr).state = TaskState::Running; }
            let next_root = unsafe { (*nptr).root_pa };
            s.current = next;
            CURRENT_PROC.store(nptr as usize, Ordering::SeqCst);
            crate::println!("[sched] before set_satp root={:#x}", next_root);
            set_satp_for(Some(next_root));
            crate::println!("[sched] after set_satp");
            unsafe { SCHED.unlock(); }
            crate::println!("[sched] starting first process (slot {})", next);
            let dummy = unsafe { &mut IDLE_CTX as *mut TaskContext };
            unsafe { crate::task::switch_to(dummy, next_ctx_ptr); }
            crate::println!("[sched] idle context resumed");
            idle_loop();
        }
        None => {
            crate::println!("[sched] no process, entering idle (net httpd)");
            idle_loop();
        }
    }
}

fn idle_loop() -> ! {
    unsafe { core::arch::asm!("csrsi sstatus, 0x2"); }
    loop {
        crate::net_stack::poll();
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
