//! Signal system calls.
use crate::abi::*;
use crate::mm::uaccess::*;
use crate::task::signal::{self, sigmask_bit};
use crate::task::{current, get_task, sched};
use crate::trap::TrapFrame;

pub fn sys_rt_sigaction(sig: i32, act: usize, oldact: usize, sigsetsize: usize) -> SysResult {
    if sigsetsize != 8 || sig < 1 || sig as usize >= NSIG {
        return Err(EINVAL);
    }
    let cur = current();
    let handlers = cur.sig();
    if oldact != 0 {
        let old = handlers.lock().actions[sig as usize];
        write_val(oldact, old)?;
    }
    if act != 0 {
        if sig == SIGKILL || sig == SIGSTOP {
            return Err(EINVAL);
        }
        let a: KSigAction = read_val(act)?;
        handlers.lock().actions[sig as usize] = a;
    }
    Ok(0)
}

pub fn sys_rt_sigprocmask(how: i32, set: usize, oldset: usize, sigsetsize: usize) -> SysResult {
    if sigsetsize != 8 {
        return Err(EINVAL);
    }
    let cur = current();
    let mut inner = cur.inner.lock();
    if oldset != 0 {
        let old = inner.sigmask;
        drop(inner);
        write_val(oldset, old)?;
        inner = cur.inner.lock();
    }
    if set != 0 {
        drop(inner);
        let s: u64 = read_val(set)?;
        inner = cur.inner.lock();
        let s = s & !(sigmask_bit(SIGKILL) | sigmask_bit(SIGSTOP));
        match how {
            SIG_BLOCK => inner.sigmask |= s,
            SIG_UNBLOCK => inner.sigmask &= !s,
            SIG_SETMASK => inner.sigmask = s,
            _ => return Err(EINVAL),
        }
    }
    Ok(0)
}

pub fn sys_rt_sigpending(set: usize, sigsetsize: usize) -> SysResult {
    if sigsetsize != 8 {
        return Err(EINVAL);
    }
    let cur = current();
    let p = {
        let inner = cur.inner.lock();
        inner.pending & inner.sigmask
    };
    write_val(set, p)?;
    Ok(0)
}

pub fn sys_rt_sigsuspend(mask: usize, sigsetsize: usize) -> SysResult {
    if sigsetsize != 8 {
        return Err(EINVAL);
    }
    let newmask: u64 = read_val(mask)?;
    let cur = current();
    {
        let mut inner = cur.inner.lock();
        let old = inner.sigmask;
        inner.saved_sigmask = Some(old);
        inner.sigmask = newmask & !(sigmask_bit(SIGKILL) | sigmask_bit(SIGSTOP));
    }
    loop {
        if signal::has_deliverable(&cur) {
            break;
        }
        sched::block_current();
    }
    // The saved mask is restored when the handler frame is built (or here if
    // the signal ends up ignored — deliver_pending handles that case by
    // leaving saved_sigmask; restore it now if still present after delivery).
    Err(EINTR)
}

pub fn sys_rt_sigtimedwait(set: usize, info: usize, timeout: usize, sigsetsize: usize) -> SysResult {
    if sigsetsize != 8 {
        return Err(EINVAL);
    }
    let want: u64 = read_val(set)?;
    let deadline = if timeout != 0 {
        let ts: Timespec = read_val(timeout)?;
        Some(crate::time::monotonic_ns() + (ts.tv_sec.max(0) as u64) * 1_000_000_000 + ts.tv_nsec as u64)
    } else {
        None
    };
    let cur = current();
    loop {
        {
            let mut inner = cur.inner.lock();
            let avail = inner.pending & want;
            if avail != 0 {
                let sig = avail.trailing_zeros() as i32 + 1;
                inner.pending &= !sigmask_bit(sig);
                let si = inner.pending_info.remove(&sig).unwrap_or(SigInfo { si_signo: sig, ..SigInfo::default() });
                drop(inner);
                if info != 0 {
                    write_val(info, si)?;
                }
                return Ok(sig as usize);
            }
            // other deliverable signals interrupt us
            if inner.pending & !inner.sigmask & !want != 0 {
                return Err(EINTR);
            }
        }
        if let Some(d) = deadline {
            if crate::time::monotonic_ns() >= d {
                return Err(EAGAIN);
            }
            crate::time::add_sleeper(&cur, d);
        }
        // Temporarily unblock the wanted signals so send_signal wakes us.
        let saved = {
            let mut inner = cur.inner.lock();
            let s = inner.sigmask;
            inner.sigmask &= !want;
            s
        };
        sched::block_current();
        cur.inner.lock().sigmask = saved;
        if deadline.is_some() {
            crate::time::remove_sleeper(&cur);
        }
    }
}

pub fn sys_rt_sigreturn(tf: &mut TrapFrame) -> SysResult {
    let cur = current();
    signal::sigreturn(&cur)?;
    Ok(tf.a0())
}

pub fn sys_sigaltstack(ss: usize, old_ss: usize) -> SysResult {
    let cur = current();
    if old_ss != 0 {
        let cur_ss = cur.inner.lock().sigaltstack;
        let mut out = cur_ss;
        if out.ss_sp == 0 {
            out.ss_flags = SS_DISABLE;
        }
        write_val(old_ss, out)?;
    }
    if ss != 0 {
        let new: StackT = read_val(ss)?;
        if new.ss_flags & SS_DISABLE != 0 {
            cur.inner.lock().sigaltstack = StackT::default();
        } else {
            if new.ss_size < 2048 {
                return Err(ENOMEM);
            }
            cur.inner.lock().sigaltstack = StackT { ss_sp: new.ss_sp, ss_flags: 0, _pad: 0, ss_size: new.ss_size };
        }
    }
    Ok(0)
}

pub fn sys_kill(pid: i32, sig: i32) -> SysResult {
    if sig < 0 || sig as usize >= NSIG {
        return Err(EINVAL);
    }
    let cur = current();
    let info = SigInfo {
        si_signo: sig,
        si_code: SI_USER,
        si_pid: cur.pid,
        si_uid: cur.uid.load(core::sync::atomic::Ordering::Relaxed),
        ..SigInfo::default()
    };
    if pid > 0 {
        let t = get_task(pid).ok_or(ESRCH)?;
        if sig != 0 {
            signal::send_signal(&t, sig, Some(info));
        }
        Ok(0)
    } else if pid == 0 {
        let pgid = cur.inner.lock().pgid;
        if sig != 0 {
            signal::kill_pgrp(pgid, sig, Some(info));
        }
        Ok(0)
    } else if pid == -1 {
        let tasks: alloc::vec::Vec<_> = crate::task::PROCESSES.lock().values().cloned().collect();
        for t in tasks {
            if t.pid != 1 && t.pid != cur.pid && sig != 0 {
                signal::send_signal(&t, sig, Some(info));
            }
        }
        Ok(0)
    } else {
        if sig != 0 && !signal::kill_pgrp(-pid, sig, Some(info)) {
            return Err(ESRCH);
        }
        Ok(0)
    }
}

pub fn sys_tkill(tid: i32, sig: i32) -> SysResult {
    sys_kill(tid, sig)
}

pub fn sys_tgkill(_tgid: i32, tid: i32, sig: i32) -> SysResult {
    sys_kill(tid, sig)
}
