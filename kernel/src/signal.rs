//! Minimal POSIX signal support: enough for nginx's process-management
//! model (master/worker signaling via `kill`, `SIGCHLD` on worker exit,
//! graceful shutdown via `SIGTERM`/`SIGQUIT`, config reload via
//! `SIGHUP`/`SIGUSR1`/`SIGUSR2`). No sigaltstack, no siginfo, no nested
//! signal stacking -- one signal in flight at a time, which is all these
//! simple one-argument `handler(int)` handlers need.

use crate::trap::TrapContext;

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

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

/// Signals whose default (no handler installed) disposition is to
/// terminate the process. Real Linux also has "terminate + core dump" and
/// "stop" dispositions, but nothing in this workload relies on the
/// distinction.
fn default_terminates(sig: usize) -> bool {
    !matches!(sig, SIGCHLD | SIGCONT | SIGURG_PLACEHOLDER)
}
const SIGURG_PLACEHOLDER: usize = 23; // SIGURG, also ignored by default; listed for clarity only.

#[derive(Clone, Copy, Default)]
pub struct SigAction {
    pub handler: usize,
    pub flags: usize,
    pub restorer: usize,
    pub mask: u64,
}

#[derive(Clone)]
pub struct SignalState {
    pub actions: [SigAction; 65],
    pub pending: u64,
    pub blocked: u64,
    /// Trap context and signal mask saved at delivery time, restored by
    /// `rt_sigreturn`. `None` means no handler is currently running.
    pub saved_cx: Option<alloc::boxed::Box<TrapContext>>,
    pub saved_blocked: u64,
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            actions: [SigAction::default(); 65],
            pending: 0,
            blocked: 0,
            saved_cx: None,
            saved_blocked: 0,
        }
    }

    pub fn raise(&mut self, sig: usize) {
        if sig >= 1 && sig <= 64 {
            self.pending |= 1 << (sig - 1);
        }
    }

    /// Pick the next deliverable signal (pending and not blocked), if any,
    /// consuming it from the pending set.
    fn take_deliverable(&mut self) -> Option<usize> {
        let deliverable = self.pending & !self.blocked;
        if deliverable == 0 {
            return None;
        }
        let sig = deliverable.trailing_zeros() as usize + 1;
        self.pending &= !(1 << (sig - 1));
        Some(sig)
    }
}

/// Outcome of checking for a deliverable signal on the current task,
/// decided while holding the task's lock; acted on afterwards so the lock
/// isn't held across `exit_current_and_run_next` (which reschedules).
pub enum SignalAction {
    None,
    Terminate(i32),
    Deliver,
}

/// Called from `trap_return` just before dropping back into user mode:
/// deliver at most one pending, unblocked signal (repeated trap_return
/// calls -- e.g. after the handler's own syscalls -- will deliver any
/// others queued behind it).
pub fn check_and_deliver() -> SignalAction {
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_lock();
    // Only one handler in flight at a time; if one's already running,
    // don't interrupt it (real Linux would push a new frame, but nothing
    // in this workload's handlers is slow enough to need that).
    if inner.signals.saved_cx.is_some() {
        return SignalAction::None;
    }
    let Some(sig) = inner.signals.take_deliverable() else {
        return SignalAction::None;
    };
    let action = inner.signals.actions[sig];
    if action.handler == SIG_IGN {
        return SignalAction::None;
    }
    if action.handler == SIG_DFL {
        if default_terminates(sig) {
            return SignalAction::Terminate(128 + sig as i32);
        }
        return SignalAction::None;
    }

    let cx = inner.trap_cx();
    inner.signals.saved_cx = Some(alloc::boxed::Box::new(*cx));
    inner.signals.saved_blocked = inner.signals.blocked;
    inner.signals.blocked |= action.mask | (1 << (sig - 1));

    cx.x[1] = action.restorer; // ra: handler's `ret` lands on the libc-provided restorer
    cx.x[10] = sig; // a0: signum, per the plain `void handler(int)` ABI
    cx.sepc = action.handler;
    SignalAction::Deliver
}

pub fn sigreturn() -> isize {
    let task = crate::task::current_task().unwrap();
    let mut inner = task.inner_lock();
    if let Some(saved) = inner.signals.saved_cx.take() {
        inner.signals.blocked = inner.signals.saved_blocked;
        *inner.trap_cx() = *saved;
    }
    // The restored context's a0 is the real return value of whatever
    // syscall the signal interrupted; trap_handler must not overwrite it,
    // so this value is discarded by the caller for this syscall specifically.
    0
}
