//! Tasks (processes), scheduler, blocking/wakeup, fork/exec/exit/wait.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::kprintln;
use crate::mm::frame;
use crate::mm::paging;
use crate::mm::vma::Mm;
use crate::fs::FdTable;

pub const TF_SIZE: usize = 536;

pub const USER_SSTATUS: usize = (1 << 5) | (1 << 18) | (3 << 13); // SPIE | SUM | FS=11

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrapFrame {
    pub regs: [usize; 32], // x0..x31
    pub sepc: usize,
    pub sstatus: usize,
    pub fp: [u64; 32], // f0..f31
    pub fcsr: u32,
    pub pad: u32,
}

impl TrapFrame {
    pub fn a0(&self) -> usize {
        self.regs[10]
    }
    pub fn a1(&self) -> usize {
        self.regs[11]
    }
    pub fn a2(&self) -> usize {
        self.regs[12]
    }
    pub fn a3(&self) -> usize {
        self.regs[13]
    }
    pub fn a4(&self) -> usize {
        self.regs[14]
    }
    pub fn a5(&self) -> usize {
        self.regs[15]
    }
    pub fn a6(&self) -> usize {
        self.regs[16]
    }
    pub fn a7(&self) -> usize {
        self.regs[17]
    }
    pub fn set_a0(&mut self, v: usize) {
        self.regs[10] = v;
    }
    pub fn sp(&self) -> usize {
        self.regs[2]
    }
    pub fn set_sp(&mut self, v: usize) {
        self.regs[2] = v;
    }
    pub fn set_pc(&mut self, v: usize) {
        self.sepc = v;
    }
    pub fn from_user(&self) -> bool {
        self.sstatus & (1 << 8) == 0
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Zombie,
    Free,
}

pub struct Task {
    pub pid: usize,
    pub parent: Option<usize>,
    pub state: TaskState,
    pub wchan: usize,
    pub kstack_top: usize,
    pub idle_stack_top: usize,
    pub ctx: usize,
    pub tf: *mut TrapFrame,
    pub mm: Mm,
    pub fds: FdTable,
    pub cwd: String,
    pub exit_code: i32,
    pub sig: crate::signal::SignalState,
    pub set_tid_address: usize,
    pub robust_list: usize,
    pub name: String,
}

pub static mut TASKS: Vec<Option<Task>> = Vec::new();
pub static mut READY: VecDeque<usize> = VecDeque::new();
pub static mut CURRENT: Option<usize> = None;
pub static mut NEXT_PID: usize = 1;

pub static mut LAST_TIMER_S0: usize = 0xdead;
pub static mut LAST_TIMER_SEPC: usize = 0;

pub fn init_tables() {
    unsafe {
        TASKS.clear();
        READY.clear();
        CURRENT = None;
        NEXT_PID = 1;
    }
}

pub fn current_pid() -> usize {
    unsafe { CURRENT.unwrap() }
}

pub fn current() -> *mut Task {
    unsafe {
        let pid = CURRENT.unwrap();
        TASKS[pid].as_mut().unwrap() as *mut Task
    }
}

pub fn task(pid: usize) -> Option<&'static mut Task> {
    unsafe { TASKS.get_mut(pid)?.as_mut() }
}

pub fn alloc_pid() -> usize {
    unsafe {
        // find a free slot
        for (i, t) in TASKS.iter_mut().enumerate() {
            if t.is_none() {
                return i;
            }
        }
        TASKS.push(None);
        TASKS.len() - 1
    }
}

// ---------- scheduling ----------

extern "C" {
    fn switch_to(cur_ctx: *mut usize, next_ctx: *mut usize);
    fn first_run_stub();
}

pub fn first_run_stub_addr() -> usize {
    first_run_stub as usize
}

fn do_switch(cur_ctx: *mut usize, next_ctx: *mut usize) {
    unsafe { switch_to(cur_ctx, next_ctx) };
}

/// Pick next task and context-switch. Caller: current is Running/Blocked/Zombie.
pub fn schedule() {
    unsafe {
        // requeue current if still runnable
        if let Some(pid) = CURRENT {
            let t = TASKS[pid].as_mut().unwrap();
            if t.state == TaskState::Running {
                t.state = TaskState::Ready;
                READY.push_back(pid);
            }
        }
        loop {
            let next = READY.pop_front();
            match next {
                Some(pid) => {
                    let cur = CURRENT;
                    let cur_state = cur.map(|p| TASKS[p].as_ref().unwrap().state);
                    if cur_state == Some(TaskState::Blocked) {
                        // current blocked; nothing more to do with it
                    }
                    if cur == Some(pid) {
                        // only task: keep running
                        TASKS[pid].as_mut().unwrap().state = TaskState::Running;
                        return;
                    }
                    let next_ctx = TASKS[pid].as_ref().unwrap().ctx;
                    let next_satp = TASKS[pid].as_ref().unwrap().mm.pt.root_ppn();
                    let next_kstack = TASKS[pid].as_ref().unwrap().kstack_top;
                    TASKS[pid].as_mut().unwrap().state = TaskState::Running;
                    if let Some(c) = cur {
                        let cur_ctx: *mut usize = &mut TASKS[c].as_mut().unwrap().ctx;
                        CURRENT = Some(pid);
                        paging::write_satp(next_satp);
                        set_sscratch(next_kstack);
                        let nctx = next_ctx;
                        do_switch(cur_ctx, &nctx as *const usize as *mut usize);
                        return;
                    } else {
                        // no current (shouldn't happen after boot)
                        CURRENT = Some(pid);
                        paging::write_satp(next_satp);
                        set_sscratch(next_kstack);
                        // enter via crafted ctx
                        let nctx = next_ctx;
                        switch_to(core::ptr::null_mut(), &nctx as *const usize as *mut usize);
                        return;
                    }
                }
                None => {
                    // idle
                    idle();
                }
            }
        }
    }
}

pub fn set_sscratch(v: usize) {
    unsafe {
        core::arch::asm!("csrw sscratch, {}", in(reg) v, options(nostack));
    }
}

pub fn get_sscratch() -> usize {
    let v: usize;
    unsafe {
        core::arch::asm!("csrr {}, sscratch", out(reg) v, options(nostack));
    }
    v
}

pub fn read_sp() -> usize {
    let v: usize;
    unsafe {
        core::arch::asm!("mv {}, sp", out(reg) v, options(nostack));
    }
    v
}

pub fn set_tp(v: usize) {
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) v, options(nostack));
    }
}

fn idle() {
    unsafe {
        // Switch to a dedicated idle stack: nested interrupts taken while
        // waiting (timer/external) must NOT clobber the blocked task's
        // trapframe or the syscall handler's stack frames.
        // The idle stack is allocated from the frame allocator so it does
        // not sit next to .data/.bss (an overflow would corrupt statics).
        let pid = CURRENT.unwrap();
        IDLE_STACK_TOP = TASKS[pid].as_ref().unwrap().idle_stack_top;
        IDLE_WORKER_TOP = TASKS[pid].as_ref().unwrap().kstack_top;
        idle_asm();
    }
}

#[no_mangle]
pub static mut IDLE_STACK_TOP: usize = 0;
#[no_mangle]
pub static mut IDLE_SAVED_SP: usize = 0;
#[no_mangle]
pub static mut IDLE_SAVED_RA: usize = 0;
#[no_mangle]
pub static mut IDLE_WORKER_TOP: usize = 0;

extern "C" {
    fn idle_asm();
}

#[no_mangle]
pub extern "C" fn ready_nonempty() -> bool {
    unsafe { !READY.is_empty() }
}

/// Block current task on a wait channel.
pub fn block_on(wchan: usize) {
    unsafe {
        let pid = CURRENT.unwrap();
        let t = TASKS[pid].as_mut().unwrap();
        t.state = TaskState::Blocked;
        t.wchan = wchan;
        schedule();
    }
}

/// Wake all tasks blocked on a wait channel.
pub fn wake_wchan(wchan: usize) {
    unsafe {
        for (pid, t) in TASKS.iter_mut().enumerate() {
            if let Some(t) = t {
                if t.state == TaskState::Blocked && t.wchan == wchan {
                    t.state = TaskState::Ready;
                    t.wchan = 0;
                    READY.push_back(pid);
                }
            }
        }
    }
}

/// Sleep for `ms` milliseconds.
pub fn sleep(ms: u64) {
    let wchan = current_pid();
    crate::timer_wheel::set_timer(crate::timer::now_ms() + ms, wchan, crate::timer_wheel::TimerKind::Wake);
    block_on(wchan);
}

// ---------- task creation ----------

pub const KSTACK_PAGES: usize = 32; // 128 KiB kernel stacks

/// Create a task shell (no user image yet).
pub fn new_task() -> usize {
    let pid = alloc_pid();
    let kstack = frame::alloc_frames(KSTACK_PAGES).expect("kstack");
    let kstack_top = kstack + KSTACK_PAGES * frame::FRAME_SIZE;
    let idle_frames = frame::alloc_frames(4).expect("idle stack");
    let idle_stack_top = idle_frames + 4 * frame::FRAME_SIZE;
    let t = Task {
        pid,
        parent: None,
        state: TaskState::Ready,
        wchan: 0,
        kstack_top,
        idle_stack_top,
        ctx: 0,
        tf: core::ptr::null_mut(),
        mm: Mm::new(),
        fds: FdTable::new(),
        cwd: "/".to_string(),
        exit_code: 0,
        sig: crate::signal::SignalState::new(),
        set_tid_address: 0,
        robust_list: 0,
        name: String::new(),
    };
    unsafe {
        TASKS[pid] = Some(t);
    }
    pid
}

/// Prepare a task to run a fresh user image: build the initial trapframe on its
/// kernel stack and a crafted switch frame so the scheduler can enter it.
pub fn set_initial_tf(pid: usize, entry: usize, user_sp: usize) {
    let t = task(pid).unwrap();
    let kstack_top = t.kstack_top;
    let tf_addr = kstack_top - TF_SIZE;
    let tf = tf_addr as *mut TrapFrame;
    unsafe {
        core::ptr::write_bytes(tf as *mut u8, 0, TF_SIZE);
        (*tf).regs[2] = user_sp;
        (*tf).sepc = entry;
        // SPP=0 (user), SPIE=1, SUM=1 so kernel can access user pages
        (*tf).sstatus = (1 << 5) | (1 << 18);
        t.tf = tf;
        // crafted switch frame: 13 slots below the tf
        let ctx_addr = tf_addr - 13 * 8;
        let ctx = ctx_addr as *mut usize;
        *ctx.add(0) = first_run_stub as usize; // ra
        for i in 1..13 {
            *ctx.add(i) = 0;
        }
        t.ctx = ctx_addr;
    }
}

extern "C" {
    fn enter_user(tf: *mut TrapFrame) -> !;
}

/// Boot path: make pid 1 the current task and enter user mode.
pub fn enter_first_task(pid: usize) -> ! {
    crate::kprintln!("[task] enter_first_task pid={}", pid);
    unsafe {
        let t = TASKS[pid].as_mut().unwrap();
        t.state = TaskState::Running;
        CURRENT = Some(pid);
        paging::write_satp(t.mm.pt.root_ppn());
        let ktop = t.kstack_top;
        set_tp(ktop);
        set_sscratch(ktop);
        let tf = t.tf;
        enter_user(tf);
    }
}

// ---------- fork / exec / exit / wait ----------

/// Fork the current task. Returns child pid (0 in child).
pub fn fork() -> isize {
    let cur_pid = current_pid();
    let (child_pid, parent) = unsafe {
        // NOTE: alloc_pid() may grow TASKS (push(None)), which REALLOCATES the
        // Vec and frees the old buffer. Any `current()` pointer captured before
        // this point is dangling afterwards. Always re-fetch AFTER alloc_pid.
        let pid = alloc_pid();
        let parent = current();
        let kstack = frame::alloc_frames(KSTACK_PAGES).expect("kstack");
        let kstack_top = kstack + KSTACK_PAGES * frame::FRAME_SIZE;
    let idle_frames = frame::alloc_frames(4).expect("idle stack");
    let idle_stack_top = idle_frames + 4 * frame::FRAME_SIZE;
        // copy mm
        let mut mm = Mm::new();
        {
            let p = &parent.as_ref().unwrap().mm;
            mm.copy_from(p);
        }
        let mut fds = parent.as_ref().unwrap().fds.clone();
        // clear epoll interests in child (each fd belongs to parent's epoll)
        for f in fds.fds.iter_mut() {
            if let Some(f) = f {
                f.epoll = None;
            }
        }
        let t = Task {
            pid,
            parent: Some(cur_pid),
            state: TaskState::Ready,
            wchan: 0,
            kstack_top,
            idle_stack_top,
            ctx: 0,
            tf: core::ptr::null_mut(),
            mm,
            fds,
            cwd: parent.as_ref().unwrap().cwd.clone(),
            exit_code: 0,
            sig: parent.as_ref().unwrap().sig.clone(),
            set_tid_address: 0,
            robust_list: 0,
            name: parent.as_ref().unwrap().name.clone(),
        };
        TASKS[pid] = Some(t);
        (pid, parent)
    };
    // child trapframe: copy of parent's, a0 = 0
    let child = task(child_pid).unwrap();
    let parent_tf = unsafe { &*(parent.as_ref().unwrap().tf) };
    let kstack_top = child.kstack_top;
    let tf_addr = kstack_top - TF_SIZE;
    let ctx_addr = tf_addr - 13 * 8;
    unsafe {
        core::ptr::copy_nonoverlapping(parent_tf as *const TrapFrame, tf_addr as *mut TrapFrame, 1);
        (*((tf_addr) as *mut TrapFrame)).set_a0(0);
        // user sp in child tf is same as parent's (its stack is a copy)
        child.tf = tf_addr as *mut TrapFrame;
        let ctx = ctx_addr as *mut usize;
        *ctx.add(0) = first_run_stub as usize;
        for i in 1..13 {
            *ctx.add(i) = 0;
        }
        child.ctx = ctx_addr;
        child.state = TaskState::Ready;
        READY.push_back(child_pid);
    }
    child_pid as isize
}

/// Exit the current task (or another via exit_group semantics).
pub fn exit(code: i32) -> ! {
    let pid = current_pid();
    exit_task(pid, code);
    schedule();
    // should never reach
    loop {}
}

pub fn exit_task(pid: usize, code: i32) {
    unsafe {
        let parent = TASKS[pid].as_ref().unwrap().parent;
        let t = TASKS[pid].as_mut().unwrap();
        t.state = TaskState::Zombie;
        t.exit_code = code;
        if let Some(p) = parent {
            // notify parent: SIGCHLD pending
            if let Some(pt) = TASKS.get_mut(p) {
                if let Some(pt) = pt.as_mut() {
                    pt.sig.pending |= 1 << 16; // SIGCHLD = 17 -> bit 16
                    if pt.state == TaskState::Blocked {
                        pt.state = TaskState::Ready;
                        pt.wchan = 0;
                        READY.push_back(p);
                    }
                }
            }
        }
    }
    crate::epoll::wake_all_epoll();
}

/// Reap a zombie child. Returns (pid, status) or None.
pub fn wait4(pid: isize, _options: i32) -> Option<(usize, i32)> {
    let cur = current_pid();
    unsafe {
        for (i, t) in TASKS.iter().enumerate() {
            if let Some(t) = t {
                if t.state == TaskState::Zombie
                    && t.parent == Some(cur)
                    && (pid == -1 || pid == i as isize || (pid == 0 && t.parent == Some(cur)))
                {
                    let code = t.exit_code;
                    let status = (code & 0xff) << 8;
                    // free resources
                    let kstack_bottom = t.kstack_top - KSTACK_PAGES * frame::FRAME_SIZE;
                    frame::free_frames(kstack_bottom, KSTACK_PAGES);
                    TASKS[i] = None;
                    return Some((i, status));
                }
            }
        }
        None
    }
}

/// Does a zombie child exist?
pub fn has_zombie() -> bool {
    let cur = current_pid();
    unsafe {
        TASKS.iter().any(|t| {
            t.as_ref().map(|t| t.state == TaskState::Zombie && t.parent == Some(cur)).unwrap_or(false)
        })
    }
}

pub fn list_tasks() {
    unsafe {
        kprintln!("=== tasks ===");
        for (i, t) in TASKS.iter().enumerate() {
            if let Some(t) = t {
                kprintln!(
                    "  pid={} state={:?} parent={:?} name={}",
                    i, t.state, t.parent, t.name
                );
            }
        }
    }
}
