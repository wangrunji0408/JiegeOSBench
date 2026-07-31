//! Signal-related syscalls.

use crate::signal::{SignalState, SIG_DFL, SIG_IGN};
use crate::syscall::{read_user, write_user};
use crate::task;

/// Linux kernel struct sigaction on riscv64: handler(8) + flags(8) + mask(8) + restorer(8)
const SIGACTION_SIZE: usize = 32;

pub fn sys_rt_sigaction(sig: usize, act: usize, oldact: usize, _sigsetsize: usize) -> isize {
    if sig == 0 || sig > 64 {
        return -22;
    }
    let t = task::current();
    let s = unsafe { &mut t.as_mut().unwrap().sig };
    if oldact != 0 {
        let mut out = [0u8; SIGACTION_SIZE];
        out[..8].copy_from_slice(&(s.handlers[sig] as u64).to_le_bytes());
        out[8..16].copy_from_slice(&(s.flags[sig] as u64).to_le_bytes());
        out[16..24].copy_from_slice(&s.mask.to_le_bytes());
        out[24..32].copy_from_slice(&0u64.to_le_bytes()); // restorer
        if write_user(oldact, &out).is_err() {
            return -14;
        }
    }
    if act != 0 {
        let data = match read_user(act, SIGACTION_SIZE) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let handler = u64::from_le_bytes(data[..8].try_into().unwrap()) as usize;
        let flags = u64::from_le_bytes(data[8..16].try_into().unwrap()) as u32;
        let mask = u64::from_le_bytes(data[16..24].try_into().unwrap());
        s.handlers[sig] = handler;
        s.flags[sig] = flags;
        if flags & crate::signal::SA_NODEFER == 0 {
            // mask includes sig by default; keep sa_mask as given (kernel ORs sig)
            s.mask = mask;
        } else {
            s.mask = mask;
        }
        // SIGKILL/SIGSTOP cannot be changed
        if sig == 9 || sig == 19 {
            s.handlers[sig] = SIG_DFL;
        }
    }
    0
}

pub fn sys_rt_sigprocmask(how: usize, set: usize, oldset: usize, _sigsetsize: usize) -> isize {
    let t = task::current();
    let s = unsafe { &mut t.as_mut().unwrap().sig };
    if oldset != 0 {
        let _ = write_user(oldset, &s.mask.to_le_bytes());
    }
    if set != 0 {
        let data = match read_user(set, 8) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let new_mask = u64::from_le_bytes(data[..8].try_into().unwrap());
        // never block SIGKILL/SIGSTOP
        let new_mask = new_mask & !((1u64 << 9) | (1u64 << 19));
        match how {
            0 => s.mask = new_mask,                       // SIG_BLOCK
            1 => s.mask &= !new_mask,                     // SIG_UNBLOCK
            2 => s.mask |= new_mask,                      // SIG_SETMASK
            _ => return -22,
        }
    }
    0
}

pub fn sys_rt_sigpending(set: usize, _sigsetsize: usize) -> isize {
    let t = task::current();
    let s = unsafe { &t.as_ref().unwrap().sig };
    let _ = write_user(set, &s.pending.to_le_bytes());
    0
}

pub fn sys_rt_sigsuspend(mask: usize, _sigsetsize: usize) -> isize {
    let t = crate::task::current();
    let s = unsafe { &mut t.as_mut().unwrap().sig };
    let old = s.mask;
    if mask != 0 {
        let data = match read_user(mask, 8) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let new_mask = u64::from_le_bytes(data[..8].try_into().unwrap());
        s.mask = new_mask & !((1u64 << 9) | (1u64 << 19));
    }
    // block until a signal arrives (send_signal wakes blocked tasks)
    let wchan = crate::task::current_pid() + 0x1000;
    crate::task::block_on(wchan);
    // restore mask and return EINTR so delivery happens
    let t = crate::task::current();
    let s = unsafe { &mut t.as_mut().unwrap().sig };
    s.mask = old;
    -4 // EINTR
}

pub fn sys_rt_sigreturn() -> isize {
    // restore from the sigframe; the trapframe is rewritten, return value unused
    let pid = task::current_pid();
    let tf = unsafe { &*(task::task(pid).unwrap().tf) };
    // sigreturn must run with the current trapframe; hand it to signal::sigreturn
    // We need a mutable pointer to the current tf: get it from the task
    let t = task::task(pid).unwrap();
    let tfp = t.tf;
    crate::signal::sigreturn(tfp);
    0
}

pub fn sys_sigaltstack(ss: usize, old_ss: usize) -> isize {
    let t = task::current();
    let s = unsafe { &mut t.as_mut().unwrap().sig };
    if old_ss != 0 {
        // struct stack_t { sp, flags, size }
        let mut out = [0u8; 24];
        out[..8].copy_from_slice(&(s.altstack_sp as u64).to_le_bytes());
        out[16..24].copy_from_slice(&(s.altstack_size as u64).to_le_bytes());
        let _ = write_user(old_ss, &out);
    }
    if ss != 0 {
        let data = match read_user(ss, 24) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let sp = u64::from_le_bytes(data[..8].try_into().unwrap()) as usize;
        let size = u64::from_le_bytes(data[16..24].try_into().unwrap()) as usize;
        s.altstack_sp = sp;
        s.altstack_size = size;
    }
    0
}

pub fn signal_state_default() -> SignalState {
    SignalState::new()
}
