use crate::mm::translated_byte_buffer;
use crate::signal::SigAction;
use crate::task::{current_task, current_user_token, suspend_current_and_run_next};

fn read_u64(token: usize, ptr: *const u8) -> u64 {
    let chunks = translated_byte_buffer(token, ptr, 8);
    let mut raw = [0u8; 8];
    let mut off = 0;
    for c in chunks {
        raw[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    u64::from_ne_bytes(raw)
}

fn write_u64(token: usize, ptr: *mut u8, val: u64) {
    if ptr.is_null() {
        return;
    }
    let raw = val.to_ne_bytes();
    let mut chunks = translated_byte_buffer(token, ptr, 8);
    let mut off = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&raw[off..off + n]);
        off += n;
    }
}

/// `struct k_sigaction { handler; flags; restorer; mask[2]; }`, 32 bytes,
/// as musl passes it to the raw `rt_sigaction` syscall.
fn read_sigaction(token: usize, ptr: *const u8) -> SigAction {
    let chunks = translated_byte_buffer(token, ptr, 32);
    let mut raw = [0u8; 32];
    let mut off = 0;
    for c in chunks {
        raw[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    SigAction {
        handler: usize::from_ne_bytes(raw[0..8].try_into().unwrap()),
        flags: usize::from_ne_bytes(raw[8..16].try_into().unwrap()),
        restorer: usize::from_ne_bytes(raw[16..24].try_into().unwrap()),
        mask: u64::from_ne_bytes(raw[24..32].try_into().unwrap()),
    }
}

fn write_sigaction(token: usize, ptr: *mut u8, action: SigAction) {
    if ptr.is_null() {
        return;
    }
    let mut raw = [0u8; 32];
    raw[0..8].copy_from_slice(&action.handler.to_ne_bytes());
    raw[8..16].copy_from_slice(&action.flags.to_ne_bytes());
    raw[16..24].copy_from_slice(&action.restorer.to_ne_bytes());
    raw[24..32].copy_from_slice(&action.mask.to_ne_bytes());
    let mut chunks = translated_byte_buffer(token, ptr, 32);
    let mut off = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&raw[off..off + n]);
        off += n;
    }
}

pub fn sys_rt_sigaction(signum: usize, act: *const u8, oldact: *mut u8, _sigsetsize: usize) -> isize {
    if signum == 0 || signum > 64 {
        return -22; // EINVAL
    }
    let token = current_user_token();
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    let old = inner.signals.actions[signum];
    if !act.is_null() {
        inner.signals.actions[signum] = read_sigaction(token, act);
    }
    drop(inner);
    write_sigaction(token, oldact, old);
    0
}

const SIG_BLOCK: usize = 0;
const SIG_UNBLOCK: usize = 1;
const SIG_SETMASK: usize = 2;

pub fn sys_rt_sigprocmask(how: usize, set: *const u8, oldset: *mut u8, _sigsetsize: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    let old = inner.signals.blocked;
    if !set.is_null() {
        let new_mask = read_u64(token, set);
        inner.signals.blocked = match how {
            SIG_BLOCK => old | new_mask,
            SIG_UNBLOCK => old & !new_mask,
            SIG_SETMASK => new_mask,
            _ => old,
        };
    }
    drop(inner);
    write_u64(token, oldset, old);
    0
}

pub fn sys_rt_sigreturn() -> isize {
    crate::signal::sigreturn()
}

pub fn sys_rt_sigsuspend(mask_ptr: *const u8) -> isize {
    let token = current_user_token();
    let new_mask = read_u64(token, mask_ptr);
    let task = current_task().unwrap();
    let old_mask = {
        let mut inner = task.inner_lock();
        let old = inner.signals.blocked;
        inner.signals.blocked = new_mask;
        old
    };
    loop {
        crate::net::poll();
        {
            let inner = task.inner_lock();
            if inner.signals.pending & !inner.signals.blocked != 0 {
                break;
            }
        }
        suspend_current_and_run_next();
    }
    task.inner_lock().signals.blocked = old_mask;
    -4 // EINTR: rt_sigsuspend always "fails" with EINTR once a signal is deliverable
}

fn raise_on(pid: usize, sig: usize) -> bool {
    let Some(target) = super::process::find_task_by_pid(pid) else {
        return false;
    };
    if sig == 0 {
        return true; // signal 0: existence check only
    }
    target.inner_lock().signals.raise(sig);
    true
}

pub fn sys_kill(pid: isize, sig: i32) -> isize {
    if pid <= 0 || sig < 0 {
        // Process-group / broadcast forms aren't needed by this workload.
        return 0;
    }
    if raise_on(pid as usize, sig as usize) {
        0
    } else {
        -3 // ESRCH
    }
}

pub fn sys_tgkill(_tgid: isize, pid: isize, sig: i32) -> isize {
    sys_kill(pid, sig)
}
