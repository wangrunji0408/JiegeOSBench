//! Threads, scheduling and the user-mode entry/exit paths.

pub mod process;

use crate::csr::*;
use crate::mm::frame;
use crate::mm::page_table::PageTable;
use crate::trap::{TaskContext, TrapFrame, TF_SIZE};
use crate::println;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::null_mut;
use process::Process;

pub const KSTACK_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Zombie,
}

pub struct Thread {
    pub tid: usize,
    pub ctx: TaskContext,
    pub tf: *mut TrapFrame,
    pub tf_frame: usize,
    pub kstack: usize,
    pub state: State,
    pub process: Option<Box<Process>>,
    /// kernel-thread entry point
    pub entry: Option<fn()>,
    /// wake-up time in ticks (0 = not sleeping)
    pub wake_tick: u64,
    /// waiting on a futex: (address space root, uaddr)
    pub futex_addr: usize,
    pub exit_code: i32,
    /// resources owned by this thread that must be released after it dies
    pub kstack_frames: usize,
    pub tf_frames: usize,
    pub clear_child_tid: usize,
    pub robust_list: usize,
    pub robust_list_len: usize,
}

unsafe impl Send for Thread {}

pub struct Scheduler {
    threads: Vec<Box<Thread>>,
    ready: Vec<usize>,
    current: usize,
    idle: usize,
    next_tid: usize,
    /// tick counter for round-robin
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

pub fn has_process() -> bool {
    current().process.is_some()
}

pub fn thread_by_tid(tid: usize) -> Option<&'static mut Thread> {
    let s = sched();
    s.threads.iter_mut().find(|t| t.tid == tid)
}

fn new_thread_skeleton(tid: usize) -> Box<Thread> {
    let tf_frame = frame::alloc_frame().expect("no frame for trap frame");
    let kstack = frame::alloc_frames(KSTACK_SIZE / 4096).expect("no frame for kernel stack");
    let tf = tf_frame as *mut TrapFrame;
    unsafe {
        core::ptr::write_bytes(tf_frame as *mut u8, 0, 4096);
    }
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
        futex_addr: 0,
        exit_code: 0,
        kstack_frames: KSTACK_SIZE / 4096,
        tf_frames: 1,
        clear_child_tid: 0,
        robust_list: 0,
        robust_list_len: 0,
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
    // thread 0: idle
    let mut idle = new_thread_skeleton(0);
    idle.ctx.ra = kernel_thread_entry as usize;
    idle.ctx.sp = idle.kstack + KSTACK_SIZE;
    idle.state = State::Ready;
    s.threads.push(idle);
    s.ready.push(0);
    s.current = 0;
    s.idle = 0;
    unsafe {
        SCHED = Some(s);
    }
}

/// Create a kernel thread running `f`.
pub fn spawn_kernel(f: fn()) -> usize {
    let s = sched();
    let tid = s.next_tid;
    s.next_tid += 1;
    let mut t = new_thread_skeleton(tid);
    t.ctx.ra = kernel_thread_entry as usize;
    t.ctx.sp = t.kstack + KSTACK_SIZE;
    t.entry = Some(f);
    t.state = State::Ready;
    let idx = s.threads.len();
    s.threads.push(t);
    s.ready.push(idx);
    tid
}

/// Create a user thread from a prepared process; returns the tid.
pub fn spawn_user(process: Process) -> usize {
    let s = sched();
    let tid = s.next_tid;
    s.next_tid += 1;
    let mut t = new_thread_skeleton(tid);
    t.ctx.ra = restore_tf_from_sp as usize;
    t.ctx.sp = t.tf as usize;
    t.process = Some(Box::new(process));
    t.state = State::Ready;
    let idx = s.threads.len();
    s.threads.push(t);
    s.ready.push(idx);
    tid
}

extern "C" fn restore_tf_from_sp() -> ! {
    // The scheduler switched to us with sp == tf.
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

fn idle_loop() -> ! {
    loop {
        // Nothing runnable: sleep until an interrupt.
        unsafe {
            enable_interrupts();
            core::arch::asm!("wfi", options(nomem, nostack));
            disable_interrupts();
        }
        schedule();
    }
}

/// Pick the next runnable thread and switch to it.
pub fn schedule() {
    unsafe {
        disable_interrupts();
    }
    let s = sched();
    let prev = s.current;
    if s.threads[prev].state == State::Running {
        s.threads[prev].state = State::Ready;
        s.ready.push(prev);
    }
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

/// Called when a thread blocks: switch away and never come back until woken.
pub fn block_current() {
    unsafe {
        disable_interrupts();
    }
    let s = sched();
    s.threads[s.current].state = State::Blocked;
    schedule();
}

/// Make a thread runnable again.
pub fn wake(pid: usize) {
    let s = sched();
    if s.threads[pid].state == State::Blocked {
        s.threads[pid].state = State::Ready;
        s.ready.push(pid);
    }
}

pub fn wake_tid(tid: usize) {
    if let Some(i) = sched().threads.iter().position(|t| t.tid == tid) {
        wake(i);
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
    if let Some(p) = s.threads[i].process.as_mut() {
        p.exit_code = code;
        p.exited = true;
    }
    crate::task::process::on_thread_exit(i);
    schedule();
    unreachable!()
}

/// Timer tick: wake sleeping threads and preempt the current user thread.
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

/// Round-robin preemption of a user thread on timer tick.
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
        crate::trap::cause_name(scause);
        crate::sbi::shutdown(true);
    }
    if crate::task::process::handle_page_fault(tf, stval, scause) {
        return;
    }
    crate::syscall::deliver_signal_fault(tf, if scause == SCAUSE_STORE_PAGE_FAULT { 7 } else { 11 });
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

pub fn user_trap(tf: &mut TrapFrame, scause: usize) {
    crate::trap::trap_handler(tf);
}
