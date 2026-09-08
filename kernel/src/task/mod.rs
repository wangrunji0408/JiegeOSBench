//! Threads, scheduling and the user-mode entry/exit paths.

pub mod elf;
pub mod process;
pub mod signal;

use crate::csr::*;
use crate::errno::*;
use crate::mm::frame;
use crate::trap::{TaskContext, TrapFrame, TF_SIZE};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use process::Process;

pub const KSTACK_SIZE: usize = 64 * 1024;

/// Root of the kernel page table (copied into every user address space).
pub static KERNEL_ROOT: AtomicUsize = AtomicUsize::new(0);

pub fn set_kernel_root(root: usize) {
    KERNEL_ROOT.store(root, Ordering::Relaxed);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Zombie,
    Dead,
}

pub struct Thread {
    pub tid: usize,
    pub ctx: TaskContext,
    pub tf: *mut TrapFrame,
    pub tf_frame: usize,
    pub kstack: usize,
    pub state: State,
    pub process: Option<Box<Process>>,
    pub entry: Option<fn()>,
    pub wake_tick: u64,
    pub exit_code: i32,
    pub kstack_frames: usize,
    pub futex_woken: bool,
    pub futex_addr: usize,
}

unsafe impl Send for Thread {}

pub struct Scheduler {
    pub threads: Vec<Box<Thread>>,
    pub ready: Vec<usize>,
    pub current: usize,
    pub idle: usize,
    pub next_tid: usize,
    pub quantum: u64,
}

static mut SCHED: Option<Scheduler> = None;

#[inline]
pub fn sched() -> &'static mut Scheduler {
    unsafe { SCHED.as_mut().unwrap() }
}

pub fn current_index() -> usize {
    sched().current
}

pub fn current() -> &'static mut Thread {
    let s = sched();
    let i = s.current;
    &mut s.threads[i]
}

pub fn current_process() -> &'static mut Process {
    current().process.as_mut().expect("no current process")
}

pub fn thread_index_by_tid(tid: usize) -> Option<usize> {
    sched().threads.iter().position(|t| t.tid == tid)
}

pub fn alloc_tid() -> usize {
    let s = sched();
    let t = s.next_tid;
    s.next_tid += 1;
    t
}

fn new_thread_skeleton(tid: usize) -> Box<Thread> {
    let tf_frame = frame::alloc_frame().expect("no frame for trap frame");
    let kstack = frame::alloc_frames(KSTACK_SIZE / 4096).expect("no frame for kernel stack");
    let tf = tf_frame as *mut TrapFrame;
    let mut t = Box::new(Thread {
        tid,
        ctx: TaskContext::empty(),
        tf,
        tf_frame,
        kstack,
        state: State::Ready,
        process: None,
        entry: None,
        wake_tick: 0,
        exit_code: 0,
        kstack_frames: KSTACK_SIZE / 4096,
        futex_woken: false,
        futex_addr: 0,
    });
    unsafe {
        (*t.tf).ksp = kstack + KSTACK_SIZE;
    }
    t
}

pub fn init() {
    let mut s = Scheduler {
        threads: Vec::new(),
        ready: Vec::new(),
        current: 0,
        idle: 0,
        next_tid: 1,
        quantum: 0,
    };
    let mut idle = new_thread_skeleton(0);
    idle.ctx.ra = kernel_thread_entry as usize;
    idle.ctx.sp = idle.kstack + KSTACK_SIZE;
    s.threads.push(idle);
    s.ready.push(0);
    unsafe {
        SCHED = Some(s);
    }
}

pub fn spawn_kernel(f: fn()) -> usize {
    let s = sched();
    let tid = s.next_tid;
    s.next_tid += 1;
    let mut t = new_thread_skeleton(tid);
    t.ctx.ra = kernel_thread_entry as usize;
    t.ctx.sp = t.kstack + KSTACK_SIZE;
    t.entry = Some(f);
    let idx = s.threads.len();
    s.threads.push(t);
    s.ready.push(idx);
    tid
}

pub fn spawn_user(process: Process) -> usize {
    let s = sched();
    let tid = process.pid;
    let mut t = new_thread_skeleton(tid);
    t.ctx.ra = restore_tf_from_sp as usize;
    t.ctx.sp = t.tf as usize;
    unsafe {
        *t.tf = process.init_tf;
        (*t.tf).ksp = t.kstack + KSTACK_SIZE;
    }
    t.process = Some(Box::new(process));
    let idx = s.threads.len();
    s.threads.push(t);
    s.ready.push(idx);
    tid
}

extern "C" fn restore_tf_from_sp() -> ! {
    unsafe {
        core::arch::asm!("j __restore_tf_from_sp", options(noreturn));
    }
}

extern "C" fn kernel_thread_entry() -> ! {
    let t = current();
    if t.tid == 0 {
        idle_loop();
    }
    let f = t.entry.take();
    if let Some(f) = f {
        f();
    }
    exit_current(0)
}

/// Boot-time entry: create the first user process.
pub fn start_init() {
    if crate::fs::initramfs_ready() {
        if let Err(e) = process::spawn_init() {
            crate::println!("[init] failed to start init: {}", e);
        }
    } else {
        crate::println!("[init] no initramfs, running boot test thread");
        spawn_kernel(boot_test);
    }
}

fn boot_test() {
    crate::println!("[test] kernel thread running, tid={}", current().tid);
    loop {
        sleep_ticks(250);
    }
}

pub fn start() -> ! {
    unsafe {
        set_sscratch(current().tf as usize);
    }
    idle_loop()
}

fn idle_loop() -> ! {
    loop {
        unsafe {
            enable_interrupts();
            core::arch::asm!("wfi", options(nomem, nostack));
            disable_interrupts();
        }
        crate::net::poll();
        schedule();
    }
}

pub fn schedule() {
    unsafe {
        disable_interrupts();
    }
    let s = sched();
    let prev = s.current;
    let next = match s.ready.pop() {
        Some(i) => i,
        None => s.idle,
    };
    if next == prev {
        s.threads[prev].state = State::Running;
        unsafe {
            enable_interrupts();
        }
        return;
    }
    if s.threads[prev].state == State::Running {
        s.threads[prev].state = State::Ready;
        s.ready.push(prev);
    }
    s.threads[next].state = State::Running;
    s.current = next;
    let next_tf = s.threads[next].tf as usize;
    let next_ctx = &s.threads[next].ctx as *const TaskContext;
    let prev_ctx = &mut s.threads[prev].ctx as *mut TaskContext;
    set_sscratch(next_tf);
    unsafe {
        crate::trap::__switch(prev_ctx, next_ctx);
    }
}

pub fn block_current() {
    unsafe {
        disable_interrupts();
    }
    let s = sched();
    s.threads[s.current].state = State::Blocked;
    schedule();
}

pub fn wake_index(i: usize) {
    let s = sched();
    if s.threads[i].state == State::Blocked {
        s.threads[i].state = State::Ready;
        s.ready.push(i);
    }
}

pub fn wake_tid(tid: usize) {
    if let Some(i) = thread_index_by_tid(tid) {
        wake_index(i);
    }
}

pub fn exit_current(code: i32) -> ! {
    unsafe {
        disable_interrupts();
    }
    let s = sched();
    let i = s.current;
    s.threads[i].exit_code = code;
    s.threads[i].state = State::Zombie;
    let (pid, ctid) = {
        match s.threads[i].process.as_mut() {
            Some(p) => {
                p.exit_code = code;
                p.exited = true;
                (p.pid, p.clear_child_tid)
            }
            None => (s.threads[i].tid, 0),
        }
    };
    if ctid != 0 {
        if let Some(p) = s.threads[i].process.as_mut() {
            let _ = p.copy_to_user(ctid, &0u32.to_le_bytes());
        }
    }
    process::notify_parent(pid, code);
    schedule();
    unreachable!()
}

pub fn on_timer() {
    let s = sched();
    let now = crate::time::ticks();
    for i in 0..s.threads.len() {
        if s.threads[i].state == State::Blocked && s.threads[i].wake_tick != 0 {
            if now >= s.threads[i].wake_tick {
                s.threads[i].wake_tick = 0;
                s.threads[i].state = State::Ready;
                s.ready.push(i);
            }
        }
    }
}

pub fn sleep_ticks(ticks: u64) {
    if ticks == 0 {
        schedule();
        return;
    }
    let s = sched();
    let i = s.current;
    s.threads[i].wake_tick = crate::time::ticks() + ticks;
    s.threads[i].state = State::Blocked;
    schedule();
}

pub fn preempt(_tf: &mut TrapFrame) {
    let s = sched();
    if s.threads[s.current].process.is_none() {
        return;
    }
    s.quantum += 1;
    if s.quantum >= 8 {
        s.quantum = 0;
        schedule();
    }
}

pub fn page_fault(tf: &mut TrapFrame, scause: usize) {
    let stval = csrr!("stval");
    if current().process.is_none() {
        crate::println!("kernel page fault at {:#x}", stval);
        crate::sbi::shutdown(true);
    }
    if process::handle_page_fault(tf, stval, scause) {
        return;
    }
    let sig = if scause == SCAUSE_STORE_PAGE_FAULT { 7 } else { 11 };
    crate::syscall::deliver_signal_fault(tf, sig);
}

pub fn fault(tf: &mut TrapFrame, scause: usize) {
    crate::println!(
        "[tid {}] user fault: {} at {:#x} pc={:#x}",
        current().tid,
        crate::trap::cause_name(scause),
        csrr!("stval"),
        tf.sepc
    );
    crate::syscall::deliver_signal_fault(tf, 7);
}

/// Called from the trap return path before going back to user mode.
pub fn deliver_signals(tf: &mut TrapFrame) {
    let t = current();
    let p = match t.process.as_mut() {
        Some(p) => p,
        None => return,
    };
    if p.sig.pending & (1 << (signal::SIGKILL - 1)) != 0 {
        crate::println!("[sig] pid {} killed", p.pid);
        exit_current(137);
    }
    signal::deliver(p, tf);
}

// ---- clone / wait / signals / futex -------------------------------------

pub fn sys_clone(
    p: &mut Process,
    flags: usize,
    stack: usize,
    ptid: usize,
    ctid: usize,
    tls: usize,
    tf: &mut TrapFrame,
) -> Result<usize, Errno> {
    const CLONE_VM: usize = 0x100;
    const CLONE_THREAD: usize = 0x10000;
    if flags & (CLONE_VM | CLONE_THREAD) != 0 {
        return Err(ENOSYS);
    }
    let _ = (stack, tls);
    let tid = alloc_tid();
    let mut child = Process::new(tid);
    Process::fork_from(p, &mut child)?;
    child.init_tf = *tf;
    child.init_tf.set_a0(0);
    child.init_tf.ksp = 0;
    if ctid != 0 {
        child.clear_child_tid = ctid;
        let _ = child.copy_to_user(ctid, &(tid as u32).to_le_bytes());
    }
    if ptid != 0 {
        let _ = p.copy_to_user(ptid, &(tid as u32).to_le_bytes());
    }
    p.children.push(tid);
    spawn_user(child);
    Ok(tid)
}

pub fn child_exit_code(pid: usize) -> Option<i32> {
    let s = sched();
    for t in s.threads.iter() {
        if t.tid == pid && (t.state == State::Zombie || t.state == State::Dead) {
            return Some(t.exit_code);
        }
    }
    None
}

pub fn reap_child(pid: usize) {
    let s = sched();
    let idx = match s.threads.iter().position(|t| t.tid == pid) {
        Some(i) => i,
        None => return,
    };
    if s.threads[idx].state == State::Dead {
        return;
    }
    if let Some(mut p) = s.threads[idx].process.take() {
        p.free_memory();
        p.files.files.clear();
    }
    s.threads[idx].state = State::Dead;
    frame::free_frames(s.threads[idx].kstack, s.threads[idx].kstack_frames);
    frame::free_frame(s.threads[idx].tf_frame);
    s.threads[idx].kstack = 0;
    s.threads[idx].tf_frame = 0;
}

pub fn send_signal(pid: i32, sig: usize, code: i32) {
    let s = sched();
    let target: Option<usize> = if pid > 0 {
        thread_index_by_tid(pid as usize)
    } else {
        let cur = &s.threads[s.current];
        let pgid = cur.process.as_ref().map(|p| p.pgid).unwrap_or(0);
        s.threads
            .iter()
            .position(|t| t.process.as_ref().map(|p| p.pgid == pgid).unwrap_or(false))
    };
    if let Some(i) = target {
        if let Some(p) = s.threads[i].process.as_mut() {
            let info = signal::SigInfo {
                code,
                pid: s.threads[s.current].tid as i32,
                uid: 0,
                ..Default::default()
            };
            p.sig.raise_with(sig, info);
            s.threads[i].futex_woken = true;
            if s.threads[i].state == State::Blocked {
                s.threads[i].state = State::Ready;
                s.ready.push(i);
            }
        }
    }
}

static FUTEX_WAITERS: crate::sync::SpinLock<Vec<(usize, usize)>> =
    crate::sync::SpinLock::new(Vec::new());

pub fn futex(
    p: &mut Process,
    uaddr: usize,
    op: i32,
    val: u32,
    _timeout: usize,
    _uaddr2: usize,
) -> Result<usize, Errno> {
    let cmd = op & 0x7f;
    match cmd {
        0 | 9 => {
            let mut b = [0u8; 4];
            p.copy_from_user(uaddr, &mut b)?;
            let cur = u32::from_le_bytes(b);
            if cur != val {
                return Err(EAGAIN);
            }
            let tid = current().tid;
            FUTEX_WAITERS.lock().push((uaddr, tid));
            current().futex_addr = uaddr;
            loop {
                let mut b = [0u8; 4];
                if p.copy_from_user(uaddr, &mut b).is_err() {
                    break;
                }
                let now = u32::from_le_bytes(b);
                if now != val || current().futex_woken {
                    break;
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    current().futex_woken = false;
                    FUTEX_WAITERS
                        .lock()
                        .retain(|(a, t)| !(*a == uaddr && *t == tid));
                    return Err(EINTR);
                }
                sleep_ticks(1);
            }
            current().futex_woken = false;
            current().futex_addr = 0;
            FUTEX_WAITERS
                .lock()
                .retain(|(a, t)| !(*a == uaddr && *t == tid));
            Ok(0)
        }
        1 | 10 => {
            let mut n = 0usize;
            let waiters: Vec<usize> = {
                let list = FUTEX_WAITERS.lock();
                list.iter()
                    .filter(|(a, _)| *a == uaddr)
                    .map(|(_, t)| *t)
                    .collect()
            };
            for tid in waiters {
                if n >= val as usize && val != u32::MAX {
                    break;
                }
                if let Some(i) = thread_index_by_tid(tid) {
                    let s = sched();
                    s.threads[i].futex_woken = true;
                    if s.threads[i].state == State::Blocked {
                        s.threads[i].state = State::Ready;
                        s.ready.push(i);
                    }
                    n += 1;
                }
            }
            Ok(n)
        }
        _ => Ok(0),
    }
}
