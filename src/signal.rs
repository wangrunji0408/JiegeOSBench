//! POSIX signals.
//!
//! nginx's master process relies on signals to control its workers, and musl
//! installs handlers for SIGCHLD/SIGPIPE, so we need real delivery: build a
//! signal frame on the user stack, run the handler, and return through
//! `rt_sigreturn`.

use crate::mm::uaccess;
use crate::task::{self, Task, TaskState};
use crate::trap::TrapContext;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

pub const SIGHUP: usize = 1;
pub const SIGINT: usize = 2;
pub const SIGQUIT: usize = 3;
pub const SIGILL: usize = 4;
pub const SIGTRAP: usize = 5;
pub const SIGABRT: usize = 6;
pub const SIGBUS: usize = 7;
pub const SIGFPE: usize = 8;
pub const SIGKILL: usize = 9;
pub const SIGUSR1: usize = 10;
pub const SIGSEGV: usize = 11;
pub const SIGUSR2: usize = 12;
pub const SIGPIPE: usize = 13;
pub const SIGALRM: usize = 14;
pub const SIGTERM: usize = 15;
pub const SIGCHLD: usize = 17;
pub const SIGCONT: usize = 18;
pub const SIGSTOP: usize = 19;
pub const SIGTSTP: usize = 20;
pub const SIGTTIN: usize = 21;
pub const SIGTTOU: usize = 22;
pub const SIGWINCH: usize = 28;
pub const SIGIO: usize = 29;
pub const SIGSYS: usize = 31;

pub const NSIG: usize = 64;

/// Special handler values.
pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

/// `sa_flags` bits.
pub const SA_NOCLDSTOP: usize = 1;
pub const SA_NOCLDWAIT: usize = 2;
pub const SA_SIGINFO: usize = 4;
pub const SA_RESTORER: usize = 0x0400_0000;
pub const SA_ONSTACK: usize = 0x0800_0000;
pub const SA_RESTART: usize = 0x1000_0000;
pub const SA_NODEFER: usize = 0x4000_0000;
pub const SA_RESETHAND: usize = 0x8000_0000;

/// A signal mask.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct SigSet(pub u64);

impl SigSet {
    pub const EMPTY: Self = Self(0);

    #[inline]
    pub fn contains(&self, sig: usize) -> bool {
        sig >= 1 && sig <= NSIG && self.0 & (1 << (sig - 1)) != 0
    }

    #[inline]
    pub fn add(&mut self, sig: usize) {
        if sig >= 1 && sig <= NSIG {
            self.0 |= 1 << (sig - 1);
        }
    }

    #[inline]
    pub fn remove(&mut self, sig: usize) {
        if sig >= 1 && sig <= NSIG {
            self.0 &= !(1 << (sig - 1));
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// The lowest set signal number.
    pub fn lowest(&self) -> Option<usize> {
        if self.0 == 0 {
            None
        } else {
            Some(self.0.trailing_zeros() as usize + 1)
        }
    }
}

/// `struct sigaction` as user space sees it (riscv64 / generic Linux).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SigAction {
    pub handler: usize,
    pub flags: usize,
    pub restorer: usize,
    pub mask: SigSet,
}

impl SigAction {
    pub fn is_ignored(&self, sig: usize) -> bool {
        self.handler == SIG_IGN || (self.handler == SIG_DFL && default_is_ignore(sig))
    }
}

/// Signals whose default action is to ignore.
fn default_is_ignore(sig: usize) -> bool {
    matches!(sig, SIGCHLD | SIGCONT | SIGWINCH | SIGURG)
}

const SIGURG: usize = 23;

/// Signals whose default action stops the process.
fn default_is_stop(sig: usize) -> bool {
    matches!(sig, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU)
}

/// Per-thread signal state.
pub struct SignalState {
    /// Signals raised but not yet delivered.
    pub pending: SigSet,
    /// Signals blocked by `sigprocmask`.
    pub mask: SigSet,
    /// Saved mask while a handler runs, restored by `rt_sigreturn`.
    saved_mask: Option<SigSet>,
    /// `siginfo` details for pending signals, keyed by signal number.
    pub info: [SigInfoData; NSIG + 1],
    /// Alternate signal stack (`sigaltstack`).
    pub altstack: SigAltStack,
}

#[derive(Clone, Copy, Default)]
pub struct SigInfoData {
    pub code: i32,
    pub pid: i32,
    pub uid: u32,
    pub value: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SigAltStack {
    pub sp: usize,
    pub flags: i32,
    pub size: usize,
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            pending: SigSet::EMPTY,
            mask: SigSet::EMPTY,
            saved_mask: None,
            info: [SigInfoData::default(); NSIG + 1],
            altstack: SigAltStack::default(),
        }
    }

    /// Is there a pending signal not blocked by the mask?
    pub fn has_deliverable(&self) -> bool {
        // SIGKILL and SIGSTOP can never be blocked.
        let unblockable = self.pending.0 & ((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
        unblockable != 0 || (self.pending.0 & !self.mask.0) != 0
    }

    /// Take the next signal to deliver.
    fn next_deliverable(&mut self) -> Option<usize> {
        let unblockable = self.pending.0 & ((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
        let candidates = if unblockable != 0 {
            unblockable
        } else {
            self.pending.0 & !self.mask.0
        };
        let sig = SigSet(candidates).lowest()?;
        self.pending.remove(sig);
        Some(sig)
    }
}

/// `struct siginfo_t`. Only the fields handlers actually read are filled in.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SigInfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    pub _pad0: i32,
    pub pid: i32,
    pub uid: u32,
    pub status: i32,
    pub _pad: [u8; 92],
}

/// The frame we push on the user stack before entering a handler.
///
/// `rt_sigreturn` reads it back to restore the interrupted context. The layout
/// only has to be self-consistent, except that `uc_mcontext` must match what
/// libc expects if a handler inspects it — musl's cancellation handler reads
/// `uc_mcontext.__gregs[REG_PC]`, so we keep the standard riscv64 layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigFrame {
    /// `siginfo_t` passed to SA_SIGINFO handlers.
    info: SigInfo,
    /// `ucontext_t`.
    uc_flags: usize,
    uc_link: usize,
    uc_stack: SigAltStack,
    uc_sigmask: SigSet,
    /// Padding so `uc_mcontext` lands where the ABI puts it (offset 176 in
    /// riscv64 `ucontext_t`).
    _pad: [u8; 120],
    /// `mcontext_t`: pc followed by x1..x31, then the FP state.
    gregs: [usize; 32],
    fpregs: [u64; 33],
    /// Our own bookkeeping, read back by `rt_sigreturn`.
    magic: usize,
    saved_sepc: usize,
    saved_sstatus: usize,
}

const SIGFRAME_MAGIC: usize = 0x5349_4746_524D_4531; // "SIGFRME1"

/// Send a signal to a specific thread.
pub fn send_to_task(task: &Arc<Task>, sig: usize) {
    if sig == 0 || sig > NSIG {
        return;
    }
    {
        let mut signals = task.signals.lock();
        signals.pending.add(sig);
    }
    // Handle SIGCONT/SIGSTOP state transitions immediately, since a stopped task
    // isn't running to notice them.
    if sig == SIGCONT {
        if task.get_state() == TaskState::Stopped {
            task::enqueue(task.clone());
        }
    }
    // Wake a blocked task so it can act on the signal.
    if task.get_state() == TaskState::Blocked {
        task::enqueue(task.clone());
    }
}

/// Send a signal to a process: deliver to the first thread that isn't blocking it.
pub fn send_to_process(task: &Arc<Task>, sig: usize) {
    if sig == 0 || sig > NSIG {
        return;
    }
    let threads: Vec<Arc<Task>> = task
        .group
        .threads
        .lock()
        .iter()
        .filter_map(|w| w.upgrade())
        .filter(|t| !t.is_zombie())
        .collect();
    if threads.is_empty() {
        return;
    }
    // Prefer a thread with the signal unblocked, as Linux does.
    let target = threads
        .iter()
        .find(|t| !t.signals.lock().mask.contains(sig))
        .unwrap_or(&threads[0]);
    send_to_task(target, sig);
}

/// Raise a signal on the current thread.
pub fn raise_current(sig: usize) {
    let task = task::current();
    send_to_task(&task, sig);
}

/// Deliver any pending signals before returning to user space.
pub fn handle_pending(cx: &mut TrapContext) {
    if !task::has_current() {
        return;
    }
    let task = task::current();

    // A group exit request beats everything else.
    if task.group.group_exiting.load(Ordering::Relaxed) && !task.is_zombie() {
        let code = task.group.exit_code.load(Ordering::Relaxed);
        task::exit_current(code);
    }

    loop {
        let sig = {
            let mut signals = task.signals.lock();
            match signals.next_deliverable() {
                Some(s) => s,
                None => return,
            }
        };

        let action = task.group.actions.lock()[sig];

        // SIGKILL and SIGSTOP cannot be caught.
        if sig == SIGKILL {
            task::exit_group(128 + sig as i32);
        }
        if sig == SIGSTOP {
            task.set_state(TaskState::Stopped);
            task::block_current();
            continue;
        }

        if action.handler == SIG_IGN {
            continue;
        }

        if action.handler == SIG_DFL {
            if default_is_ignore(sig) {
                continue;
            }
            if default_is_stop(sig) {
                task.set_state(TaskState::Stopped);
                task::block_current();
                continue;
            }
            // Default action: terminate.
            crate::info!(
                "pid {} terminated by signal {} ({})",
                task.pid(),
                sig,
                signal_name(sig)
            );
            task::exit_group(128 + sig as i32);
        }

        // Run the user handler.
        if !setup_frame(&task, cx, sig, &action) {
            crate::warn!(
                "pid {} could not deliver signal {}; terminating",
                task.pid(),
                sig
            );
            task::exit_group(128 + sig as i32);
        }
        // Only one handler per return to user space; the rest stay pending.
        return;
    }
}

/// Build the signal frame and redirect the context into the handler.
fn setup_frame(task: &Arc<Task>, cx: &mut TrapContext, sig: usize, action: &SigAction) -> bool {
    let signals_info = task.signals.lock().info[sig];
    let old_mask = task.signals.lock().mask;

    // Choose the stack: the alternate one if requested and set up.
    let altstack = task.signals.lock().altstack;
    let on_altstack = action.flags & SA_ONSTACK != 0 && altstack.sp != 0 && altstack.size != 0;
    let base_sp = if on_altstack {
        altstack.sp + altstack.size
    } else {
        // Skip the 128-byte red zone the ABI reserves below sp.
        cx.sp() - 128
    };

    let frame_size = core::mem::size_of::<SigFrame>();
    // 16-byte align the frame.
    let frame_addr = (base_sp - frame_size) & !0xf;

    let mut frame = SigFrame {
        info: SigInfo {
            signo: sig as i32,
            errno: 0,
            code: signals_info.code,
            _pad0: 0,
            pid: signals_info.pid,
            uid: signals_info.uid,
            status: 0,
            _pad: [0; 92],
        },
        uc_flags: 0,
        uc_link: 0,
        uc_stack: altstack,
        uc_sigmask: old_mask,
        _pad: [0; 120],
        gregs: [0; 32],
        fpregs: [0; 33],
        magic: SIGFRAME_MAGIC,
        saved_sepc: cx.sepc,
        saved_sstatus: cx.sstatus,
    };
    // mcontext: gregs[0] is pc, gregs[1..32] are x1..x31.
    frame.gregs[0] = cx.sepc;
    frame.gregs[1..32].copy_from_slice(&cx.x[1..32]);
    frame.fpregs[..32].copy_from_slice(&cx.f);
    frame.fpregs[32] = cx.fcsr as u64;

    if uaccess::write(frame_addr, frame).is_err() {
        return false;
    }

    // Block this signal (unless SA_NODEFER) plus the handler's mask while it runs.
    {
        let mut signals = task.signals.lock();
        signals.saved_mask = Some(old_mask);
        let mut new_mask = old_mask;
        new_mask.0 |= action.mask.0;
        if action.flags & SA_NODEFER == 0 {
            new_mask.add(sig);
        }
        signals.mask = new_mask;
    }

    // SA_RESETHAND: restore the default disposition before running.
    if action.flags & SA_RESETHAND != 0 {
        task.group.actions.lock()[sig] = SigAction::default();
    }

    // Enter the handler: a0 = signo, a1 = &siginfo, a2 = &ucontext.
    cx.sepc = action.handler;
    cx.set_sp(frame_addr);
    cx.x[10] = sig; // a0
    cx.x[11] = frame_addr + core::mem::offset_of!(SigFrame, info); // a1
    cx.x[12] = frame_addr + core::mem::offset_of!(SigFrame, uc_flags); // a2
    // Return address: the restorer musl provides, which issues `rt_sigreturn`.
    cx.x[1] = if action.restorer != 0 {
        action.restorer
    } else {
        // No restorer: point ra at the trampoline we install in every process.
        sigreturn_trampoline()
    };
    true
}

/// `rt_sigreturn`: restore the pre-signal context.
///
/// Returns the value to leave in a0 (the interrupted syscall's return value).
pub fn sigreturn(cx: &mut TrapContext) -> isize {
    let task = task::current();
    // The frame sits at the current sp, where the handler's prologue left it.
    let frame_addr = cx.sp();
    let frame: SigFrame = match uaccess::read(frame_addr) {
        Ok(f) => f,
        Err(_) => {
            crate::warn!("rt_sigreturn: bad frame at {:#x}", frame_addr);
            task::exit_group(128 + SIGSEGV as i32);
        }
    };
    if frame.magic != SIGFRAME_MAGIC {
        crate::warn!(
            "rt_sigreturn: bad magic {:#x} at {:#x}",
            frame.magic,
            frame_addr
        );
        task::exit_group(128 + SIGSEGV as i32);
    }

    // Restore registers. A handler may have edited the mcontext (some libcs do),
    // so take the values from there rather than our saved copies.
    cx.sepc = frame.gregs[0];
    cx.x[1..32].copy_from_slice(&frame.gregs[1..32]);
    cx.f.copy_from_slice(&frame.fpregs[..32]);
    cx.fcsr = frame.fpregs[32] as usize;
    cx.sstatus = frame.saved_sstatus;

    // Restore the mask the handler ran under.
    {
        let mut signals = task.signals.lock();
        signals.mask = frame.uc_sigmask;
        signals.saved_mask = None;
    }

    // a0 was restored from the frame; return it so the dispatcher doesn't
    // clobber it.
    cx.x[10] as isize
}

/// Address of the in-kernel sigreturn trampoline mapped into every process.
///
/// musl always supplies `sa_restorer`, but a handler installed directly via the
/// raw syscall might not, so we keep a fallback: a page mapped in user space
/// containing `li a7, 139; ecall`.
static mut TRAMPOLINE_ADDR: usize = 0;

pub fn set_trampoline(addr: usize) {
    unsafe { TRAMPOLINE_ADDR = addr };
}

fn sigreturn_trampoline() -> usize {
    unsafe { TRAMPOLINE_ADDR }
}

pub fn signal_name(sig: usize) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        20 => "SIGTSTP",
        28 => "SIGWINCH",
        29 => "SIGIO",
        31 => "SIGSYS",
        _ => "SIG?",
    }
}

/// Record `siginfo` for a signal we are about to raise.
pub fn set_info(task: &Arc<Task>, sig: usize, code: i32, pid: i32, uid: u32) {
    if sig <= NSIG {
        let mut signals = task.signals.lock();
        signals.info[sig] = SigInfoData {
            code,
            pid,
            uid,
            value: 0,
        };
    }
}
