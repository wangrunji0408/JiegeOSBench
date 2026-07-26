//! The task (thread) structure.
//!
//! We model Linux's process/thread split: every schedulable entity is a `Task`
//! with its own `tid`. Tasks in one thread group share a `tgid` (the pid), the
//! address space, the fd table, and the signal handlers. `fork` makes a new
//! thread group; `clone(CLONE_THREAD)` adds to the existing one.

use crate::fs::{FdTable, File, InodeRef};
use crate::mm::{AddrSpace, KERNEL_STACK_SIZE};
use crate::signal::{SigAction, SigSet, SignalState};
use crate::trap::{TaskContext, TrapContext};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use spin::{Mutex, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// On the run queue or currently running.
    Runnable,
    /// Blocked in the kernel (waiting on a futex, child, or I/O).
    Blocked,
    /// Stopped by SIGSTOP.
    Stopped,
    /// Exited; waiting to be reaped.
    Zombie,
}

/// State shared by every thread in a thread group.
pub struct ThreadGroup {
    pub tgid: usize,
    /// The thread group leader's ppid.
    pub ppid: AtomicUsize,
    /// Process group ID, for signal delivery.
    pub pgid: AtomicUsize,
    /// Session ID.
    pub sid: AtomicUsize,
    /// Live threads in this group.
    pub threads: Mutex<Vec<Weak<Task>>>,
    /// Signal dispositions, shared process-wide.
    pub actions: Mutex<[SigAction; 65]>,
    /// The exit code once the group has exited.
    pub exit_code: AtomicI32,
    /// Set once any thread called `exit_group`, so the others wind down.
    pub group_exiting: AtomicBool,
    /// Child processes, for `wait4`.
    pub children: Mutex<Vec<Arc<Task>>>,
    /// Real uid/gid and effective uid/gid. nginx drops privileges to `nobody`
    /// when started as root, so these have to change and stick.
    pub uid: AtomicU32,
    pub euid: AtomicU32,
    pub gid: AtomicU32,
    pub egid: AtomicU32,
    /// Supplementary groups.
    pub groups: Mutex<Vec<u32>>,
    pub umask: AtomicU32,
    /// The executable's path, for `/proc/self/exe`.
    pub exe: RwLock<String>,
    /// argv, for `/proc/self/cmdline` and `ps`.
    pub cmdline: RwLock<Vec<String>>,
    /// Accumulated CPU time in ticks.
    pub utime: AtomicUsize,
    pub stime: AtomicUsize,
}

impl ThreadGroup {
    fn new(tgid: usize, ppid: usize) -> Arc<Self> {
        Arc::new(Self {
            tgid,
            ppid: AtomicUsize::new(ppid),
            pgid: AtomicUsize::new(tgid),
            sid: AtomicUsize::new(tgid),
            threads: Mutex::new(Vec::new()),
            actions: Mutex::new([SigAction::default(); 65]),
            exit_code: AtomicI32::new(0),
            group_exiting: AtomicBool::new(false),
            children: Mutex::new(Vec::new()),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            groups: Mutex::new(Vec::new()),
            umask: AtomicU32::new(0o022),
            exe: RwLock::new(String::new()),
            cmdline: RwLock::new(Vec::new()),
            utime: AtomicUsize::new(0),
            stime: AtomicUsize::new(0),
        })
    }
}

/// The kernel stack for a task. Boxed so its address is stable, and the trap
/// context lives at the top of it.
pub struct KernelStack {
    data: Box<[u8]>,
}

impl KernelStack {
    fn new() -> Self {
        // `vec![0; N].into_boxed_slice()` allocates without a huge stack temp.
        let data = alloc::vec![0u8; KERNEL_STACK_SIZE].into_boxed_slice();
        Self { data }
    }

    /// The address just past the end of the stack.
    fn top(&self) -> usize {
        self.data.as_ptr() as usize + self.data.len()
    }

    /// Carve the trap context out of the top of the stack and return a pointer
    /// to it, plus the kernel sp to use below it.
    fn setup_context(&self) -> (*mut TrapContext, usize) {
        let ctx_size = core::mem::size_of::<TrapContext>();
        // Keep the context 16-byte aligned, as the ABI requires for sp.
        let ctx_addr = (self.top() - ctx_size) & !0xf;
        (ctx_addr as *mut TrapContext, ctx_addr)
    }
}

pub struct Task {
    /// Thread ID; unique across the system.
    pub tid: usize,
    /// Shared thread-group state.
    pub group: Arc<ThreadGroup>,
    /// The address space, shared with other threads in the group.
    pub aspace: Arc<Mutex<AddrSpace>>,
    /// Open files, shared with other threads in the group (and with a child
    /// created by `clone(CLONE_FILES)`).
    pub files: Arc<Mutex<FdTable>>,
    /// Current working directory, shared within the group.
    pub cwd: Arc<Mutex<InodeRef>>,
    /// Scheduler state.
    pub state: Mutex<TaskState>,
    /// Kernel stack; owns the memory the trap context lives in.
    kstack: KernelStack,
    /// Pointer to the trap context at the top of the kernel stack.
    trap_cx: AtomicUsize,
    /// Saved kernel context for `__switch`.
    pub task_cx: Mutex<TaskContext>,
    /// Per-thread signal state.
    pub signals: Mutex<SignalState>,
    /// Where to write the tid on clone/exit (`CLONE_CHILD_CLEARTID`).
    pub clear_child_tid: AtomicUsize,
    pub set_child_tid: AtomicUsize,
    /// Thread name (`prctl(PR_SET_NAME)`), also used for `/proc/self/stat`.
    pub comm: RwLock<String>,
    /// Exit code for this thread.
    pub exit_code: AtomicI32,
    /// Robust futex list head, stored so `set_robust_list` succeeds.
    pub robust_list: AtomicUsize,
    /// `RLIMIT_STACK` and friends that we track but mostly ignore.
    pub rlimits: Mutex<[crate::fs::stat::RLimit; 16]>,
}

/// tid allocation.
static NEXT_TID: AtomicUsize = AtomicUsize::new(1);
static PROCESSES_CREATED: AtomicUsize = AtomicUsize::new(0);

fn alloc_tid() -> usize {
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}

pub fn processes_created() -> usize {
    PROCESSES_CREATED.load(Ordering::Relaxed)
}

impl Task {
    /// Create the first user process.
    pub fn new_init(aspace: AddrSpace, entry: usize, user_sp: usize) -> Arc<Self> {
        let tid = alloc_tid();
        let group = ThreadGroup::new(tid, 0);
        let kstack = KernelStack::new();
        let (cx_ptr, kernel_sp) = kstack.setup_context();
        unsafe {
            cx_ptr.write(TrapContext::new_user(entry, user_sp, kernel_sp));
        }

        let mut files = FdTable::new();
        // Wire up stdin/stdout/stderr to the console.
        let tty = crate::fs::device::new_tty();
        let stdin = Arc::new(File::with_path(
            tty.clone(),
            crate::fs::OpenFlags::RDONLY,
            "/dev/console",
        ));
        let stdout = Arc::new(File::with_path(
            tty.clone(),
            crate::fs::OpenFlags::WRONLY,
            "/dev/console",
        ));
        let stderr = Arc::new(File::with_path(
            tty,
            crate::fs::OpenFlags::WRONLY,
            "/dev/console",
        ));
        let _ = files.insert(stdin, false);
        let _ = files.insert(stdout, false);
        let _ = files.insert(stderr, false);

        let task = Arc::new(Self {
            tid,
            group: group.clone(),
            aspace: Arc::new(Mutex::new(aspace)),
            files: Arc::new(Mutex::new(files)),
            cwd: Arc::new(Mutex::new(crate::fs::root().clone())),
            state: Mutex::new(TaskState::Runnable),
            kstack,
            trap_cx: AtomicUsize::new(cx_ptr as usize),
            task_cx: Mutex::new(TaskContext::new(
                task_entry as usize,
                kernel_sp - core::mem::size_of::<TrapContext>(),
            )),
            signals: Mutex::new(SignalState::new()),
            clear_child_tid: AtomicUsize::new(0),
            set_child_tid: AtomicUsize::new(0),
            comm: RwLock::new("init".to_string()),
            exit_code: AtomicI32::new(0),
            robust_list: AtomicUsize::new(0),
            rlimits: Mutex::new(default_rlimits()),
        });
        group.threads.lock().push(Arc::downgrade(&task));
        PROCESSES_CREATED.fetch_add(1, Ordering::Relaxed);
        task
    }

    /// The trap context for this task.
    #[allow(clippy::mut_from_ref)]
    pub fn trap_context(&self) -> &'static mut TrapContext {
        unsafe { &mut *(self.trap_cx.load(Ordering::Relaxed) as *mut TrapContext) }
    }

    pub fn trap_context_ptr(&self) -> *mut TrapContext {
        self.trap_cx.load(Ordering::Relaxed) as *mut TrapContext
    }

    pub fn pid(&self) -> usize {
        self.group.tgid
    }

    pub fn ppid(&self) -> usize {
        self.group.ppid.load(Ordering::Relaxed)
    }

    pub fn pgid(&self) -> usize {
        self.group.pgid.load(Ordering::Relaxed)
    }

    pub fn name(&self) -> String {
        self.comm.read().clone()
    }

    pub fn exe_path(&self) -> String {
        self.group.exe.read().clone()
    }

    pub fn cmdline(&self) -> Vec<String> {
        self.group.cmdline.read().clone()
    }

    pub fn is_group_leader(&self) -> bool {
        self.tid == self.group.tgid
    }

    /// Fork: a new thread group with a copy-on-write clone of this address space.
    pub fn fork(self: &Arc<Self>, flags: CloneFlags, child_stack: usize) -> Option<Arc<Task>> {
        let tid = alloc_tid();
        let thread = flags.contains(CloneFlags::THREAD);

        let group = if thread {
            self.group.clone()
        } else {
            let g = ThreadGroup::new(tid, self.pid());
            g.pgid.store(self.pgid(), Ordering::Relaxed);
            g.sid.store(self.group.sid.load(Ordering::Relaxed), Ordering::Relaxed);
            *g.actions.lock() = *self.group.actions.lock();
            g.uid.store(self.group.uid.load(Ordering::Relaxed), Ordering::Relaxed);
            g.euid.store(self.group.euid.load(Ordering::Relaxed), Ordering::Relaxed);
            g.gid.store(self.group.gid.load(Ordering::Relaxed), Ordering::Relaxed);
            g.egid.store(self.group.egid.load(Ordering::Relaxed), Ordering::Relaxed);
            *g.groups.lock() = self.group.groups.lock().clone();
            g.umask.store(self.group.umask.load(Ordering::Relaxed), Ordering::Relaxed);
            *g.exe.write() = self.group.exe.read().clone();
            *g.cmdline.write() = self.group.cmdline.read().clone();
            g
        };

        // Address space: shared for threads and `CLONE_VM`, COW-copied otherwise.
        let aspace = if flags.contains(CloneFlags::VM) || thread {
            self.aspace.clone()
        } else {
            let forked = self.aspace.lock().fork()?;
            Arc::new(Mutex::new(forked))
        };

        let files = if flags.contains(CloneFlags::FILES) {
            self.files.clone()
        } else {
            Arc::new(Mutex::new(self.files.lock().clone_for_fork()))
        };

        let cwd = if flags.contains(CloneFlags::FS) {
            self.cwd.clone()
        } else {
            Arc::new(Mutex::new(self.cwd.lock().clone()))
        };

        let kstack = KernelStack::new();
        let (cx_ptr, kernel_sp) = kstack.setup_context();
        // The child resumes from the same place with a0 = 0.
        unsafe {
            let mut cx = *self.trap_context();
            cx.kernel_sp = kernel_sp;
            cx.set_return(0);
            if child_stack != 0 {
                cx.set_sp(child_stack);
            }
            if flags.contains(CloneFlags::SETTLS) {
                // tls value was stashed by the caller.
            }
            cx_ptr.write(cx);
        }

        let child = Arc::new(Self {
            tid,
            group: group.clone(),
            aspace,
            files,
            cwd,
            state: Mutex::new(TaskState::Runnable),
            kstack,
            trap_cx: AtomicUsize::new(cx_ptr as usize),
            task_cx: Mutex::new(TaskContext::new(
                task_entry as usize,
                kernel_sp - core::mem::size_of::<TrapContext>(),
            )),
            signals: Mutex::new(SignalState::new()),
            clear_child_tid: AtomicUsize::new(0),
            set_child_tid: AtomicUsize::new(0),
            comm: RwLock::new(self.comm.read().clone()),
            exit_code: AtomicI32::new(0),
            robust_list: AtomicUsize::new(0),
            rlimits: Mutex::new(*self.rlimits.lock()),
        });

        group.threads.lock().push(Arc::downgrade(&child));
        if !thread {
            // Register with the parent for `wait4`.
            self.group.children.lock().push(child.clone());
            PROCESSES_CREATED.fetch_add(1, Ordering::Relaxed);
        }
        Some(child)
    }

    /// Replace this task's image (`execve`). Returns the new entry point and sp.
    pub fn set_trap_context(&self, cx: TrapContext) {
        let (cx_ptr, kernel_sp) = self.kstack.setup_context();
        let mut cx = cx;
        cx.kernel_sp = kernel_sp;
        unsafe { cx_ptr.write(cx) };
        self.trap_cx.store(cx_ptr as usize, Ordering::Relaxed);
    }

    pub fn kernel_sp(&self) -> usize {
        self.kstack.top()
    }

    pub fn set_state(&self, state: TaskState) {
        *self.state.lock() = state;
    }

    pub fn get_state(&self) -> TaskState {
        *self.state.lock()
    }

    pub fn is_zombie(&self) -> bool {
        *self.state.lock() == TaskState::Zombie
    }

    /// uid/gid accessors.
    pub fn uid(&self) -> u32 {
        self.group.uid.load(Ordering::Relaxed)
    }
    pub fn euid(&self) -> u32 {
        self.group.euid.load(Ordering::Relaxed)
    }
    pub fn gid(&self) -> u32 {
        self.group.gid.load(Ordering::Relaxed)
    }
    pub fn egid(&self) -> u32 {
        self.group.egid.load(Ordering::Relaxed)
    }
    pub fn umask(&self) -> u32 {
        self.group.umask.load(Ordering::Relaxed)
    }
}

fn default_rlimits() -> [crate::fs::stat::RLimit; 16] {
    use crate::fs::stat::{RLimit, RLIM_INFINITY};
    let mut limits = [RLimit {
        cur: RLIM_INFINITY,
        max: RLIM_INFINITY,
    }; 16];
    // RLIMIT_STACK
    limits[3] = RLimit {
        cur: crate::mm::USER_STACK_SIZE as u64,
        max: RLIM_INFINITY,
    };
    // RLIMIT_NOFILE
    limits[7] = RLimit {
        cur: crate::fs::fdtable::DEFAULT_NOFILE as u64,
        max: crate::fs::fdtable::MAX_NOFILE as u64,
    };
    // RLIMIT_NPROC
    limits[6] = RLimit {
        cur: 4096,
        max: 4096,
    };
    limits
}

bitflags::bitflags! {
    /// Linux `CLONE_*` flags.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct CloneFlags: usize {
        const CSIGNAL        = 0x000000ff;
        const VM             = 0x00000100;
        const FS             = 0x00000200;
        const FILES          = 0x00000400;
        const SIGHAND        = 0x00000800;
        const PIDFD          = 0x00001000;
        const PTRACE         = 0x00002000;
        const VFORK          = 0x00004000;
        const PARENT         = 0x00008000;
        const THREAD         = 0x00010000;
        const NEWNS          = 0x00020000;
        const SYSVSEM        = 0x00040000;
        const SETTLS         = 0x00080000;
        const PARENT_SETTID  = 0x00100000;
        const CHILD_CLEARTID = 0x00200000;
        const DETACHED       = 0x00400000;
        const UNTRACED       = 0x00800000;
        const CHILD_SETTID   = 0x01000000;
        const NEWCGROUP      = 0x02000000;
        const NEWUTS         = 0x04000000;
        const NEWIPC         = 0x08000000;
        const NEWUSER        = 0x10000000;
        const NEWPID         = 0x20000000;
        const NEWNET         = 0x40000000;
        const IO             = 0x80000000;
    }
}

/// The kernel-side entry point for a freshly scheduled task: return to user.
extern "C" fn task_entry() -> ! {
    // Interrupts were disabled by the scheduler across `__switch`.
    crate::trap::enable_interrupts();
    let cx = super::sched::current_trap_context();
    unsafe { crate::trap::__trap_return(cx) }
}

/// Dump user registers, for diagnosing faults.
pub fn dump_user_context(cx: &TrapContext) {
    const NAMES: [&str; 32] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    crate::println!("  pc = {:#018x}", cx.sepc);
    for row in 0..8 {
        let mut line = String::new();
        for col in 0..4 {
            let i = row * 4 + col;
            line.push_str(&alloc::format!("{:>4}={:#018x} ", NAMES[i], cx.x[i]));
        }
        crate::println!("  {}", line);
    }
    // Show which VMA the faulting pc lies in, which quickly tells us whether we
    // are in the executable, the linker, or a library.
    let task = super::sched::current();
    let aspace = task.aspace.lock();
    if let Some(vma) = aspace.find_vma(cx.sepc) {
        crate::println!(
            "  pc in {} [{:#x}..{:#x}) +{:#x}",
            vma.name,
            vma.start,
            vma.end,
            cx.sepc - vma.start
        );
    }
}
