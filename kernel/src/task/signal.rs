//! Signal generation and delivery.
use alloc::sync::Arc;

use super::{process, sched, Task, TaskState};
use crate::abi::*;
use crate::config::SIGRET_TRAMPOLINE;
use crate::mm::uaccess::{read_val_mm, write_val_mm};

#[inline]
pub fn sigmask_bit(sig: i32) -> u64 {
    1u64 << (sig - 1)
}

/// Signals whose default action is to ignore.
fn default_ignored(sig: i32) -> bool {
    matches!(sig, SIGCHLD | SIGURG | SIGWINCH | SIGCONT)
}

/// Signals whose default action stops the process (we treat as ignore).
fn default_stop(sig: i32) -> bool {
    matches!(sig, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU)
}

/// Does the task currently ignore `sig` (so it can be dropped at send time)?
fn sig_ignored(task: &Task, sig: i32) -> bool {
    if sig == SIGKILL || sig == SIGSTOP {
        return false;
    }
    let inner = task.inner.lock();
    if inner.sigmask & sigmask_bit(sig) != 0 {
        return false;
    }
    drop(inner);
    let sig_h = task.sig();
    let act = sig_h.lock().actions[sig as usize];
    match act.handler {
        SIG_IGN => true,
        SIG_DFL => default_ignored(sig) || default_stop(sig),
        _ => false,
    }
}

pub fn send_signal(task: &Arc<Task>, sig: i32, info: Option<SigInfo>) {
    if sig <= 0 || sig as usize >= NSIG {
        return;
    }
    if sig_ignored(task, sig) {
        return;
    }
    let mut inner = task.inner.lock();
    if inner.state == TaskState::Zombie {
        return;
    }
    inner.pending |= sigmask_bit(sig);
    if let Some(i) = info {
        inner.pending_info.insert(sig, i);
    }
    let wake = inner.sigmask & sigmask_bit(sig) == 0 || sig == SIGKILL;
    let blocked = inner.state == TaskState::Blocked;
    if wake && blocked {
        inner.interrupted = true;
    }
    drop(inner);
    if wake && blocked {
        sched::make_runnable(task);
    }
}

pub fn has_deliverable(task: &Task) -> bool {
    let inner = task.inner.lock();
    inner.pending & !inner.sigmask != 0
}

pub fn has_pending_unblocked(task: &Task) -> bool {
    has_deliverable(task)
}

fn take_signal(task: &Task) -> Option<(i32, SigInfo)> {
    let mut inner = task.inner.lock();
    let deliverable = inner.pending & !inner.sigmask;
    if deliverable == 0 {
        return None;
    }
    // SIGKILL first, otherwise lowest numbered
    let sig = if deliverable & sigmask_bit(SIGKILL) != 0 { SIGKILL } else { deliverable.trailing_zeros() as i32 + 1 };
    inner.pending &= !sigmask_bit(sig);
    let info = inner.pending_info.remove(&sig).unwrap_or_else(|| SigInfo {
        si_signo: sig,
        si_code: SI_KERNEL,
        ..SigInfo::default()
    });
    Some((sig, info))
}

/// Deliver at most one pending signal to the current task (called on the way
/// back to user mode). May terminate the task.
pub fn deliver_pending(task: &Arc<Task>) {
    let Some((sig, info)) = take_signal(task) else {
        // No signal: if a syscall wants restarting (interrupted by a signal that
        // turned out to be ignored), do it now.
        restart_syscall_if_needed(task, None);
        return;
    };
    let action = task.sig().lock().actions[sig as usize];
    if action.handler == SIG_IGN && sig != SIGKILL && sig != SIGSTOP {
        restart_syscall_if_needed(task, None);
        return;
    }
    if action.handler == SIG_DFL || sig == SIGKILL || sig == SIGSTOP {
        if default_ignored(sig) || default_stop(sig) {
            restart_syscall_if_needed(task, None);
            return;
        }
        klog!("pid {} ({}) killed by signal {}", task.pid, task.name(), sig);
        process::exit_current(sig & 0x7f);
    }
    // Run the handler.
    restart_syscall_if_needed(task, Some(&action));
    setup_frame(task, sig, &info, &action);
}

/// Handle the "interrupted syscall" state left by the dispatcher.
fn restart_syscall_if_needed(task: &Arc<Task>, action: Option<&KSigAction>) {
    let mut inner = task.inner.lock();
    if action.is_none() {
        // No handler frame will carry the saved mask (sigsuspend): restore it.
        if let Some(m) = inner.saved_sigmask.take() {
            inner.sigmask = m;
        }
    }
    let Some((orig_a0, kind)) = inner.syscall_restart.take() else { return };
    drop(inner);
    let tf = task.tf();
    let do_restart = match action {
        None => true, // no handler runs: always restart transparently
        Some(a) => kind == RestartKind::Always && a.flags & SA_RESTART != 0,
    };
    if do_restart {
        tf.set_a0(orig_a0);
        tf.sepc -= 4;
    } else {
        tf.set_a0((-EINTR) as isize as usize);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RestartKind {
    /// Restart if SA_RESTART or no handler (ERESTARTSYS).
    Always,
    /// Restart only if no handler is invoked (ERESTARTNOHAND).
    NoHand,
}

fn setup_frame(task: &Arc<Task>, sig: i32, info: &SigInfo, action: &KSigAction) {
    let tf = task.tf();
    let mm = task.mm();
    let (mask_to_save, altstack) = {
        let mut inner = task.inner.lock();
        let saved = inner.saved_sigmask.take().unwrap_or(inner.sigmask);
        (saved, inner.sigaltstack)
    };

    // Choose the stack.
    let mut sp = tf.sp();
    if action.flags & SA_ONSTACK != 0 && altstack.ss_flags & SS_DISABLE == 0 && altstack.ss_sp != 0 {
        let on_alt = sp > altstack.ss_sp && sp <= altstack.ss_sp + altstack.ss_size;
        if !on_alt {
            sp = altstack.ss_sp + altstack.ss_size;
        }
    }
    let frame_size = core::mem::size_of::<RtSigFrame>();
    sp = (sp - frame_size) & !15;

    let mut uc = UContext {
        uc_flags: 0,
        uc_link: 0,
        uc_stack: altstack,
        uc_sigmask: mask_to_save,
        _unused: [0; 128],
        sc_regs: [0; 32],
        sc_fpregs: tf.f,
        sc_fcsr: tf.fcsr as u32,
        _fpad: 0,
        _fres: [0; 33],
    };
    uc.sc_regs[0] = tf.sepc;
    uc.sc_regs[1..32].copy_from_slice(&tf.x[1..32]);
    let frame = RtSigFrame { info: *info, uc };
    if write_val_mm(&mm, sp, frame).is_err() {
        klog!("pid {}: cannot write signal frame at {:#x}; killing", task.pid, sp);
        process::exit_current(SIGSEGV);
    }

    // Update the mask.
    {
        let mut inner = task.inner.lock();
        inner.sigmask |= action.mask;
        if action.flags & SA_NODEFER == 0 {
            inner.sigmask |= sigmask_bit(sig);
        }
        inner.sigmask &= !(sigmask_bit(SIGKILL) | sigmask_bit(SIGSTOP));
    }
    if action.flags & SA_RESETHAND != 0 {
        task.sig().lock().actions[sig as usize] = KSigAction::default();
    }

    let info_addr = sp + core::mem::offset_of!(RtSigFrame, info);
    let uc_addr = sp + core::mem::offset_of!(RtSigFrame, uc);
    tf.sepc = action.handler;
    tf.set_sp(sp);
    tf.x[10] = sig as usize;
    tf.x[11] = info_addr;
    tf.x[12] = uc_addr;
    tf.x[1] = SIGRET_TRAMPOLINE; // ra
}

/// rt_sigreturn: restore state from the frame at the user stack pointer.
pub fn sigreturn(task: &Arc<Task>) -> Result<(), i32> {
    let tf = task.tf();
    let mm = task.mm();
    let sp = tf.sp();
    let frame: RtSigFrame = read_val_mm(&mm, sp)?;
    let uc = &frame.uc;
    tf.sepc = uc.sc_regs[0];
    tf.x[1..32].copy_from_slice(&uc.sc_regs[1..32]);
    tf.f = uc.sc_fpregs;
    tf.fcsr = uc.sc_fcsr as usize;
    let mut inner = task.inner.lock();
    inner.sigmask = uc.uc_sigmask & !(sigmask_bit(SIGKILL) | sigmask_bit(SIGSTOP));
    Ok(())
}

/// Send a signal to every task in process group `pgid`.
pub fn kill_pgrp(pgid: i32, sig: i32, info: Option<SigInfo>) -> bool {
    let tasks: alloc::vec::Vec<Arc<Task>> = super::PROCESSES.lock().values().cloned().collect();
    let mut any = false;
    for t in tasks {
        if t.inner.lock().pgid == pgid {
            any = true;
            send_signal(&t, sig, info);
        }
    }
    any
}
