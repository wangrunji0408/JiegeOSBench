//! Signal syscalls.

use crate::fs::Result;
use crate::mm::uaccess;
use crate::signal::{self, SigAction, SigAltStack, SigInfo, SigSet, NSIG};
use crate::{bail, task};

pub fn sys_rt_sigaction(
    sig: usize,
    act_ptr: usize,
    old_ptr: usize,
    sigsetsize: usize,
) -> Result<isize> {
    if sig == 0 || sig > NSIG || sigsetsize != 8 {
        bail!(EINVAL);
    }
    // SIGKILL and SIGSTOP cannot have handlers installed.
    if act_ptr != 0 && (sig == signal::SIGKILL || sig == signal::SIGSTOP) {
        bail!(EINVAL);
    }
    let task = task::current();

    if old_ptr != 0 {
        let old = task.group.actions.lock()[sig];
        uaccess::write(old_ptr, old)?;
    }
    if act_ptr != 0 {
        let new: SigAction = uaccess::read(act_ptr)?;
        task.group.actions.lock()[sig] = new;
    }
    Ok(0)
}

const SIG_BLOCK: i32 = 0;
const SIG_UNBLOCK: i32 = 1;
const SIG_SETMASK: i32 = 2;

pub fn sys_rt_sigprocmask(
    how: i32,
    set_ptr: usize,
    old_ptr: usize,
    sigsetsize: usize,
) -> Result<isize> {
    if sigsetsize != 8 {
        bail!(EINVAL);
    }
    let task = task::current();

    if old_ptr != 0 {
        let old = task.signals.lock().mask;
        uaccess::write(old_ptr, old)?;
    }
    if set_ptr != 0 {
        let set: SigSet = uaccess::read(set_ptr)?;
        let mut signals = task.signals.lock();
        match how {
            SIG_BLOCK => signals.mask.0 |= set.0,
            SIG_UNBLOCK => signals.mask.0 &= !set.0,
            SIG_SETMASK => signals.mask = set,
            _ => bail!(EINVAL),
        }
        // SIGKILL and SIGSTOP can never be blocked.
        signals.mask.remove(signal::SIGKILL);
        signals.mask.remove(signal::SIGSTOP);
    }
    Ok(0)
}

pub fn sys_rt_sigpending(set_ptr: usize, sigsetsize: usize) -> Result<isize> {
    if sigsetsize != 8 {
        bail!(EINVAL);
    }
    let task = task::current();
    let pending = task.signals.lock().pending;
    uaccess::write(set_ptr, pending)?;
    Ok(0)
}

pub fn sys_rt_sigsuspend(set_ptr: usize, sigsetsize: usize) -> Result<isize> {
    if sigsetsize != 8 {
        bail!(EINVAL);
    }
    let mask: SigSet = uaccess::read(set_ptr)?;
    let task = task::current();

    // Swap in the temporary mask and wait for a signal that it permits.
    let saved = {
        let mut signals = task.signals.lock();
        let saved = signals.mask;
        signals.mask = mask;
        signals.mask.remove(signal::SIGKILL);
        signals.mask.remove(signal::SIGSTOP);
        saved
    };

    while !task::has_pending_signal() {
        task::yield_now();
    }

    task.signals.lock().mask = saved;
    // `sigsuspend` always reports EINTR.
    bail!(EINTR)
}

pub fn sys_rt_sigtimedwait(
    set_ptr: usize,
    info_ptr: usize,
    timeout_ptr: usize,
    sigsetsize: usize,
) -> Result<isize> {
    if sigsetsize != 8 {
        bail!(EINVAL);
    }
    let wanted: SigSet = uaccess::read(set_ptr)?;
    let deadline = if timeout_ptr != 0 {
        let ts: crate::fs::stat::Timespec = uaccess::read(timeout_ptr)?;
        Some(crate::time::monotonic_ms() + (ts.sec as u64) * 1000 + (ts.nsec as u64) / 1_000_000)
    } else {
        None
    };
    let task = task::current();

    loop {
        // Take a matching pending signal without running its handler.
        let taken = {
            let mut signals = task.signals.lock();
            let candidates = signals.pending.0 & wanted.0;
            match SigSet(candidates).lowest() {
                Some(sig) => {
                    signals.pending.remove(sig);
                    Some(sig)
                }
                None => None,
            }
        };
        if let Some(sig) = taken {
            if info_ptr != 0 {
                let data = task.signals.lock().info[sig];
                let info = SigInfo {
                    signo: sig as i32,
                    errno: 0,
                    code: data.code,
                    _pad0: 0,
                    pid: data.pid,
                    uid: data.uid,
                    status: 0,
                    _pad: [0; 92],
                };
                uaccess::write(info_ptr, info)?;
            }
            return Ok(sig as isize);
        }

        if let Some(deadline) = deadline {
            if crate::time::monotonic_ms() >= deadline {
                bail!(EAGAIN);
            }
        }
        // A signal outside `wanted` interrupts the wait.
        if task::has_pending_signal() {
            bail!(EINTR);
        }
        task::yield_now();
    }
}

pub fn sys_sigaltstack(new_ptr: usize, old_ptr: usize) -> Result<isize> {
    let task = task::current();
    if old_ptr != 0 {
        let old = task.signals.lock().altstack;
        uaccess::write(old_ptr, old)?;
    }
    if new_ptr != 0 {
        let new: SigAltStack = uaccess::read(new_ptr)?;
        // SS_DISABLE is flag bit 1.
        const SS_DISABLE: i32 = 2;
        const MINSIGSTKSZ: usize = 2048;
        if new.flags & SS_DISABLE != 0 {
            task.signals.lock().altstack = SigAltStack::default();
        } else {
            if new.size < MINSIGSTKSZ {
                bail!(EINVAL);
            }
            task.signals.lock().altstack = new;
        }
    }
    Ok(0)
}

pub fn sys_kill(pid: isize, sig: usize) -> Result<isize> {
    if sig > NSIG {
        bail!(EINVAL);
    }
    let task = task::current();
    let uid = task.euid();
    let sender_pid = task.pid() as i32;

    match pid {
        // Every process in our process group.
        0 => {
            let targets = task::tasks_in_pgroup(task.pgid());
            for target in targets {
                if sig != 0 {
                    signal::set_info(&target, sig, 0, sender_pid, uid);
                    signal::send_to_process(&target, sig);
                }
            }
            Ok(0)
        }
        // Every process we may signal (except init).
        -1 => {
            for target in task::all_tasks() {
                if target.pid() > 1 && target.is_group_leader() && sig != 0 {
                    signal::set_info(&target, sig, 0, sender_pid, uid);
                    signal::send_to_process(&target, sig);
                }
            }
            Ok(0)
        }
        p if p > 0 => {
            let target = task::find_process(p as usize).ok_or(crate::err!(ESRCH))?;
            // Signal 0 is an existence check.
            if sig != 0 {
                signal::set_info(&target, sig, 0, sender_pid, uid);
                signal::send_to_process(&target, sig);
            }
            Ok(0)
        }
        // Process group -pid.
        p => {
            let pgid = (-p) as usize;
            let targets = task::tasks_in_pgroup(pgid);
            if targets.is_empty() {
                bail!(ESRCH);
            }
            for target in targets {
                if sig != 0 {
                    signal::set_info(&target, sig, 0, sender_pid, uid);
                    signal::send_to_process(&target, sig);
                }
            }
            Ok(0)
        }
    }
}

pub fn sys_tkill(tid: usize, sig: usize) -> Result<isize> {
    if sig > NSIG {
        bail!(EINVAL);
    }
    let target = task::find_task(tid).ok_or(crate::err!(ESRCH))?;
    if sig != 0 {
        let task = task::current();
        signal::set_info(&target, sig, 0, task.pid() as i32, task.euid());
        signal::send_to_task(&target, sig);
    }
    Ok(0)
}

pub fn sys_tgkill(tgid: usize, tid: usize, sig: usize) -> Result<isize> {
    if sig > NSIG {
        bail!(EINVAL);
    }
    let target = task::find_task(tid).ok_or(crate::err!(ESRCH))?;
    if target.pid() != tgid {
        bail!(ESRCH);
    }
    if sig != 0 {
        let task = task::current();
        signal::set_info(&target, sig, 0, task.pid() as i32, task.euid());
        signal::send_to_task(&target, sig);
    }
    Ok(0)
}
