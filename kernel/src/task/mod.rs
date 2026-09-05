//! Tasks (processes).
pub mod elf;
pub mod exec;
pub mod process;
pub mod sched;
pub mod signal;
pub mod wait;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use crate::abi::*;
use crate::config::*;
use crate::fs::fdtable::FdTable;
use crate::fs::vfs::Dentry;
use crate::mm::addrspace::AddressSpace;
use crate::sync::{Global, SpinLock};
use crate::trap::{Context, TrapFrame};

pub type Pid = i32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskState {
    Runnable,
    Running,
    Blocked,
    Zombie,
}

pub struct SigHandlers {
    pub actions: [KSigAction; NSIG],
}

impl SigHandlers {
    pub fn new() -> Self {
        SigHandlers { actions: [KSigAction::default(); NSIG] }
    }
}

/// Mutable, lock-protected part of a task.
pub struct TaskInner {
    pub state: TaskState,
    pub parent: Weak<Task>,
    pub children: Vec<Arc<Task>>,
    pub exit_code: i32,
    pub name: String,
    pub cwd: Arc<Dentry>,
    pub umask: u32,
    pub pgid: Pid,
    pub sid: Pid,
    pub clear_child_tid: usize,
    pub robust_list: usize,
    // signals
    pub sigmask: u64,
    pub pending: u64,
    pub pending_info: BTreeMap<i32, SigInfo>,
    pub saved_sigmask: Option<u64>,
    pub sigaltstack: StackT,
    pub rlimits: [Rlimit; 16],
    /// Set when the task was woken by a signal while blocked.
    pub interrupted: bool,
    pub exe_path: String,
}

pub struct Task {
    pub pid: Pid,
    pub tgid: Pid,
    tf: UnsafeCell<TrapFrame>,
    ctx: UnsafeCell<Context>,
    kstack: Vec<u8>,
    mm: SpinLock<Arc<SpinLock<AddressSpace>>>,
    pub fds: SpinLock<Arc<SpinLock<FdTable>>>,
    pub sig: SpinLock<Arc<SpinLock<SigHandlers>>>,
    pub inner: SpinLock<TaskInner>,
    pub in_kernel_since: AtomicUsize,
    pub utime: AtomicUsize,
    pub stime: AtomicUsize,
    pub last_enter: AtomicUsize,
    pub uid: AtomicU32,
    pub gid: AtomicU32,
    pub exit_signal: AtomicI32,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

static NEXT_PID: AtomicI32 = AtomicI32::new(1);
pub static PROCESSES: SpinLock<BTreeMap<Pid, Arc<Task>>> = SpinLock::new(BTreeMap::new());
pub static CURRENT: Global<Option<Arc<Task>>> = Global::new();

pub fn alloc_pid() -> Pid {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

pub fn default_rlimits() -> [Rlimit; 16] {
    let mut r = [Rlimit { cur: RLIM_INFINITY, max: RLIM_INFINITY }; 16];
    r[RLIMIT_NOFILE as usize] = Rlimit { cur: 1024, max: 4096 };
    r[RLIMIT_STACK as usize] = Rlimit { cur: USER_STACK_SIZE as u64, max: RLIM_INFINITY };
    r[RLIMIT_NPROC as usize] = Rlimit { cur: 4096, max: 4096 };
    r[RLIMIT_CORE as usize] = Rlimit { cur: 0, max: RLIM_INFINITY };
    r
}

impl Task {
    /// Create a task with a fresh kernel stack. `entry` is where the kernel
    /// context starts executing when first switched to.
    pub fn new(
        pid: Pid,
        name: String,
        mm: Arc<SpinLock<AddressSpace>>,
        fds: Arc<SpinLock<FdTable>>,
        sig: Arc<SpinLock<SigHandlers>>,
        cwd: Arc<Dentry>,
        parent: Weak<Task>,
    ) -> Arc<Task> {
        let kstack = alloc::vec![0u8; KSTACK_SIZE];
        let kstack_top = (kstack.as_ptr() as usize + KSTACK_SIZE) & !15;
        let mut ctx = Context::zero();
        ctx.ra = process::forkret as usize;
        ctx.sp = kstack_top;
        let mut tf = TrapFrame::default();
        tf.kernel_sp = kstack_top;
        Arc::new(Task {
            pid,
            tgid: pid,
            tf: UnsafeCell::new(tf),
            ctx: UnsafeCell::new(ctx),
            kstack,
            mm: SpinLock::new(mm),
            fds: SpinLock::new(fds),
            sig: SpinLock::new(sig),
            inner: SpinLock::new(TaskInner {
                state: TaskState::Runnable,
                parent,
                children: Vec::new(),
                exit_code: 0,
                name,
                cwd,
                umask: 0o022,
                pgid: pid,
                sid: pid,
                clear_child_tid: 0,
                robust_list: 0,
                sigmask: 0,
                pending: 0,
                pending_info: BTreeMap::new(),
                saved_sigmask: None,
                sigaltstack: StackT::default(),
                rlimits: default_rlimits(),
                interrupted: false,
                exe_path: String::new(),
            }),
            in_kernel_since: AtomicUsize::new(0),
            utime: AtomicUsize::new(0),
            stime: AtomicUsize::new(0),
            last_enter: AtomicUsize::new(0),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            exit_signal: AtomicI32::new(SIGCHLD),
        })
    }

    pub fn kstack_top(&self) -> usize {
        (self.kstack.as_ptr() as usize + KSTACK_SIZE) & !15
    }

    #[allow(clippy::mut_from_ref)]
    pub fn tf(&self) -> &mut TrapFrame {
        unsafe { &mut *self.tf.get() }
    }

    pub fn tf_ptr(&self) -> *mut TrapFrame {
        self.tf.get()
    }

    pub fn ctx_ptr(&self) -> *mut Context {
        self.ctx.get()
    }

    pub fn mm(&self) -> Arc<SpinLock<AddressSpace>> {
        self.mm.lock().clone()
    }

    pub fn set_mm(&self, mm: Arc<SpinLock<AddressSpace>>) {
        *self.mm.lock() = mm;
    }

    pub fn fds(&self) -> Arc<SpinLock<FdTable>> {
        self.fds.lock().clone()
    }

    pub fn sig(&self) -> Arc<SpinLock<SigHandlers>> {
        self.sig.lock().clone()
    }

    pub fn name(&self) -> String {
        self.inner.lock().name.clone()
    }

    pub fn state(&self) -> TaskState {
        self.inner.lock().state
    }

    pub fn set_state(&self, s: TaskState) {
        self.inner.lock().state = s;
    }

    pub fn cwd(&self) -> Arc<Dentry> {
        self.inner.lock().cwd.clone()
    }

    pub fn ppid(&self) -> Pid {
        self.inner.lock().parent.upgrade().map(|p| p.pid).unwrap_or(0)
    }

    pub fn stats_enter_kernel(&self) {
        let now = crate::time::monotonic_ns() as usize;
        let last = self.last_enter.swap(now, Ordering::Relaxed);
        if last != 0 {
            self.utime.fetch_add(now - last, Ordering::Relaxed);
        }
    }

    pub fn stats_leave_kernel(&self) {
        let now = crate::time::monotonic_ns() as usize;
        let last = self.last_enter.swap(now, Ordering::Relaxed);
        if last != 0 {
            self.stime.fetch_add(now.saturating_sub(last), Ordering::Relaxed);
        }
    }
}

/// The currently running task. Panics in the idle context.
pub fn current() -> Arc<Task> {
    CURRENT.get().as_ref().expect("no current task").clone()
}

pub fn try_current() -> Option<Arc<Task>> {
    if CURRENT.is_init() {
        CURRENT.get().clone()
    } else {
        None
    }
}

pub fn get_task(pid: Pid) -> Option<Arc<Task>> {
    PROCESSES.lock().get(&pid).cloned()
}

pub fn init() {
    CURRENT.init(None);
}
