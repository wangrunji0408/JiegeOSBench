//! 进程管理与调度

use crate::config::{KERNEL_STACK_SIZE, PAGE_SIZE, TRAP_CONTEXT};
use crate::fd::Fd;
use crate::mm::{AddressSpace, MapPerm, PhysPageNum, VirtAddr};
use crate::signal::SigAction;
use crate::sync::UPIntrFreeCell;
use crate::trap::{trap_return, TrapContext, CTX_SIZE};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::global_asm;

global_asm!(
    r#"
    .section .text
    .globl __switch
    .align 2
__switch:
    # a0 = current *mut TaskContext, a1 = next *const TaskContext
    sd ra, 0(a0)
    sd sp, 8(a0)
    sd s0, 16(a0)
    sd s1, 24(a0)
    sd s2, 32(a0)
    sd s3, 40(a0)
    sd s4, 48(a0)
    sd s5, 56(a0)
    sd s6, 64(a0)
    sd s7, 72(a0)
    sd s8, 80(a0)
    sd s9, 88(a0)
    sd s10, 96(a0)
    sd s11, 104(a0)
    ld ra, 0(a1)
    ld sp, 8(a1)
    ld s0, 16(a1)
    ld s1, 24(a1)
    ld s2, 32(a1)
    ld s3, 40(a1)
    ld s4, 48(a1)
    ld s5, 56(a1)
    ld s6, 64(a1)
    ld s7, 72(a1)
    ld s8, 80(a1)
    ld s9, 88(a1)
    ld s10, 96(a1)
    ld s11, 104(a1)
    ret
"#
);

extern "C" {
    fn __switch(current: *mut TaskContext, next: *const TaskContext);
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskState {
    Ready,
    Running,
    Sleeping,
    Blocked,
    Zombie,
}

/// 内核栈：从堆分配的对齐缓冲区（物理连续、恒等映射）
pub struct KernelStack {
    pub ptr: *mut u8,
    pub size: usize,
}

unsafe impl Send for KernelStack {}

impl KernelStack {
    pub fn new() -> Self {
        let layout =
            core::alloc::Layout::from_size_align(KERNEL_STACK_SIZE, PAGE_SIZE).unwrap();
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        assert!(!ptr.is_null(), "kernel stack alloc failed");
        Self {
            ptr,
            size: KERNEL_STACK_SIZE,
        }
    }
    pub fn top(&self) -> usize {
        self.ptr as usize + self.size
    }
    /// TrapContext 位于内核栈顶页尾部
    pub fn trap_cx(&self) -> &'static mut TrapContext {
        unsafe { &mut *((self.top() - CTX_SIZE) as *mut TrapContext) }
    }
    pub fn trap_cx_pa(&self) -> usize {
        self.top() - CTX_SIZE
    }
    pub fn top_page_ppn(&self) -> PhysPageNum {
        PhysPageNum((self.top() - PAGE_SIZE) / PAGE_SIZE)
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        let layout =
            core::alloc::Layout::from_size_align(KERNEL_STACK_SIZE, PAGE_SIZE).unwrap();
        unsafe { alloc::alloc::dealloc(self.ptr, layout) };
    }
}

pub struct Task {
    pub pid: usize,
    /// 调度上下文（Arc 堆分配，地址稳定；单核下直接裸指针访问）
    pub ctx: UPIntrFreeCell<TaskContext>,
    pub inner: UPIntrFreeCell<TaskInner>,
}

pub struct TaskInner {
    pub state: TaskState,
    pub space: AddressSpace,
    pub kstack: KernelStack,
    pub fd_table: Vec<Option<Arc<Fd>>>,
    pub cwd: String,
    pub exe: String,
    pub brk_start: usize,
    pub brk: usize,
    pub mmap_top: usize,
    pub parent: Option<alloc::sync::Weak<Task>>,
    pub children: Vec<Arc<Task>>,
    pub exit_code: i32,
    pub sig_actions: [SigAction; 65],
    pub sig_mask: u64,
    pub clear_child_tid: usize,
    pub rlimit_nofile: u64,
    pub rlimit_stack: u64,
    pub name: String,
    pub sleep_until_us: u64,
}

impl Task {
    pub fn user_satp(&self) -> usize {
        self.inner.lock().space.token()
    }
    pub fn trap_cx(&self) -> &'static mut TrapContext {
        self.inner.lock().kstack.trap_cx()
    }
    pub fn state(&self) -> TaskState {
        self.inner.lock().state
    }

    /// 分配一个 fd 槽位
    pub fn alloc_fd(&self, fd: Arc<Fd>) -> usize {
        self.alloc_fd_from(0, fd)
    }

    pub fn alloc_fd_from(&self, start: usize, fd: Arc<Fd>) -> usize {
        let mut inner = self.inner.lock();
        let limit = inner.rlimit_nofile as usize;
        for i in start..inner.fd_table.len() {
            if inner.fd_table[i].is_none() {
                inner.fd_table[i] = Some(fd);
                return i;
            }
        }
        if inner.fd_table.len() < limit {
            inner.fd_table.push(Some(fd));
            inner.fd_table.len() - 1
        } else {
            usize::MAX // EMFILE 由调用方处理
        }
    }

    pub fn get_fd(&self, fd: usize) -> Option<Arc<Fd>> {
        self.inner.lock().fd_table.get(fd).and_then(|f| f.clone())
    }

    pub fn close_fd(&self, fd: usize) {
        let mut inner = self.inner.lock();
        if fd < inner.fd_table.len() {
            if let Some(f) = inner.fd_table[fd].take() {
                crate::fd::epoll_remove_fd(fd);
                if let crate::fd::FdKind::Epoll(id) = &f.kind {
                    crate::fd::epoll_close(*id);
                }
                if let crate::fd::FdKind::Socket(id) = &f.kind {
                    crate::net::close_socket(*id);
                }
            }
        }
    }
}

lazy_static::lazy_static! {
    static ref READY_QUEUE: UPIntrFreeCell<VecDeque<Arc<Task>>> =
        unsafe { UPIntrFreeCell::new(VecDeque::new()) };
    static ref ALL_TASKS: UPIntrFreeCell<BTreeMap<usize, Arc<Task>>> =
        unsafe { UPIntrFreeCell::new(BTreeMap::new()) };
    static ref CURRENT: UPIntrFreeCell<Option<Arc<Task>>> =
        unsafe { UPIntrFreeCell::new(None) };
}

static mut IDLE_CTX: TaskContext = TaskContext::zero();
static mut NEXT_PID: usize = 1;

fn alloc_pid() -> usize {
    unsafe {
        let p = NEXT_PID;
        NEXT_PID += 1;
        p
    }
}

pub fn current_task() -> Option<Arc<Task>> {
    CURRENT.lock().clone()
}

pub fn get_task(pid: usize) -> Option<Arc<Task>> {
    ALL_TASKS.lock().get(&pid).cloned()
}

pub fn remove_task(pid: usize) {
    ALL_TASKS.lock().remove(&pid);
}

/// 创建一个新任务骨架（地址空间已准备好）
pub fn new_task(space: AddressSpace, name: String) -> Arc<Task> {
    let pid = alloc_pid();
    let kstack = KernelStack::new();
    let cx_pa = kstack.trap_cx_pa();
    let mut inner = TaskInner {
        state: TaskState::Ready,
        space,
        kstack,
        fd_table: Vec::new(),
        cwd: String::from("/"),
        exe: name.clone(),
        brk_start: 0,
        brk: 0,
        mmap_top: crate::config::MMAP_BASE,
        parent: None,
        children: Vec::new(),
        exit_code: 0,
        sig_actions: [SigAction::default(); 65],
        sig_mask: 0,
        clear_child_tid: 0,
        rlimit_nofile: 65536,
        rlimit_stack: 8 * 1024 * 1024,
        name,
        sleep_until_us: 0,
    };
    // 映射内核栈顶页到 TRAP_CONTEXT
    let kstack_top_ppn = inner.kstack.top_page_ppn();
    inner.space.map_page_at(
        VirtAddr(TRAP_CONTEXT),
        kstack_top_ppn,
        MapPerm::R | MapPerm::W,
    );
    // 初始化 TrapContext 公共字段
    let cx = inner.kstack.trap_cx();
    *cx = TrapContext::zero();
    cx.sstatus = crate::trap::user_sstatus();
    cx.kernel_satp = AddressSpace::kernel_token();
    cx.kernel_sp = cx_pa;
    cx.trap_handler = crate::trap::trap_handler_addr();
    // 调度上下文：首次调度跳到 forkret
    let mut ctx = TaskContext::zero();
    ctx.ra = forkret as usize;
    ctx.sp = cx_pa - 64; // TrapContext 之下

    let task = Arc::new(Task {
        pid,
        ctx: unsafe { UPIntrFreeCell::new(ctx) },
        inner: unsafe { UPIntrFreeCell::new(inner) },
    });
    ALL_TASKS.lock().insert(pid, task.clone());
    task
}

extern "C" fn forkret() -> ! {
    let task = current_task().expect("forkret: no current task");
    let satp = task.user_satp();
    let cx = task.trap_cx();
    println!(
        "[task] entering user pid={} entry={:#x} sp={:#x}",
        task.pid, cx.sepc, cx.x[2]
    );
    trap_return(cx, satp)
}

/// 将就绪任务入队
pub fn add_to_queue(task: Arc<Task>) {
    READY_QUEUE.lock().push_back(task);
}

/// 当前任务主动让出 CPU
pub fn schedule() {
    let task = match current_task() {
        Some(t) => t,
        None => return,
    };
    {
        let mut inner = task.inner.lock();
        if inner.state == TaskState::Running {
            inner.state = TaskState::Ready;
            drop(inner);
            READY_QUEUE.lock().push_back(task.clone());
        }
    }
    unsafe {
        __switch(task.ctx.as_ptr(), core::ptr::addr_of!(IDLE_CTX) as *const _);
    }
}

/// 当前任务阻塞（等待事件，由 idle_poll 唤醒重检）
pub fn block_current() {
    let task = current_task().expect("block: no current");
    task.inner.lock().state = TaskState::Blocked;
    schedule();
}

/// 当前任务睡眠到指定时间（us）
pub fn sleep_current_until(deadline_us: u64) {
    let task = current_task().expect("sleep: no current");
    {
        let mut inner = task.inner.lock();
        inner.state = TaskState::Sleeping;
        inner.sleep_until_us = deadline_us;
    }
    schedule();
}

/// 当前进程退出
pub fn exit_current(code: i32) -> ! {
    let task = current_task().expect("exit: no current");
    let pid = task.pid;
    println!("[task] pid={} exit code={}", pid, code);
    {
        let mut inner = task.inner.lock();
        inner.state = TaskState::Zombie;
        inner.exit_code = code;
        inner.fd_table.clear();
        // 释放地址空间帧
        inner.space.areas.clear();
    }
    schedule();
    unreachable!("zombie task resumed");
}

/// 唤醒睡眠/阻塞任务
fn wake_tasks() {
    let now = crate::timer::get_time_us();
    let tasks: Vec<Arc<Task>> = ALL_TASKS.lock().values().cloned().collect();
    for task in tasks {
        let mut inner = task.inner.lock();
        match inner.state {
            TaskState::Sleeping => {
                if now >= inner.sleep_until_us {
                    inner.state = TaskState::Ready;
                    drop(inner);
                    READY_QUEUE.lock().push_back(task);
                }
            }
            TaskState::Blocked => {
                inner.state = TaskState::Ready;
                drop(inner);
                READY_QUEUE.lock().push_back(task);
            }
            _ => {}
        }
    }
}

fn idle_poll() {
    crate::net::poll();
    // 等待下一个 tick（wfi 会被定时器中断唤醒，即使 SIE=0）
    unsafe { core::arch::asm!("wfi") };
    crate::timer::set_next_trigger();
    crate::net::poll();
    wake_tasks();
}

pub fn run_tasks() -> ! {
    println!("scheduler started");
    loop {
        let task = READY_QUEUE.lock().pop_front();
        match task {
            Some(task) => {
                task.inner.lock().state = TaskState::Running;
                *CURRENT.lock() = Some(task.clone());
                unsafe {
                    __switch(
                        core::ptr::addr_of_mut!(IDLE_CTX),
                        task.ctx.as_ptr() as *const TaskContext,
                    );
                }
                *CURRENT.lock() = None;
            }
            None => {
                idle_poll();
            }
        }
    }
}

/// 初始化：加载 init 进程
pub fn init() {
    let init_path = "/sbin/init";
    let data = crate::fs::with_fs(|fs| {
        match fs.lookup(init_path, "/", true) {
            Ok(id) => Some(fs.nodes[id].data.clone()),
            Err(_) => None,
        }
    });
    let data = match data {
        Some(d) => d,
        None => panic!("init not found at {}", init_path),
    };
    let args = alloc::vec![String::from(init_path)];
    let envs = alloc::vec![
        String::from("PATH=/usr/sbin:/usr/bin:/sbin:/bin"),
        String::from("HOME=/"),
    ];
    let task = match crate::elf::exec_task(&data, args, envs) {
        Ok(t) => t,
        Err(e) => panic!("failed to exec init: {}", e),
    };
    add_to_queue(task);
    println!("init process loaded");
}
