//! Process-related system calls.
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::abi::*;
use crate::mm::uaccess::*;
use crate::task::{current, get_task, process, sched};

pub fn sys_exit(code: i32) -> SysResult {
    process::exit_current((code & 0xff) << 8)
}

pub fn sys_exit_group(code: i32) -> SysResult {
    process::exit_current((code & 0xff) << 8)
}

pub fn sys_set_tid_address(ptr: usize) -> SysResult {
    let cur = current();
    cur.inner.lock().clear_child_tid = ptr;
    Ok(cur.pid as usize)
}

pub fn sys_set_robust_list(head: usize, _len: usize) -> SysResult {
    current().inner.lock().robust_list = head;
    Ok(0)
}

pub fn sys_clone(flags: u64, stack: usize, ptid: usize, tls: usize, ctid: usize) -> SysResult {
    if flags & CLONE_THREAD != 0 || flags & CLONE_VM != 0 {
        // Threads are not supported: pretend we are out of resources.
        klog!("clone: thread creation requested (flags={:#x}) - unsupported", flags);
        return Err(EAGAIN);
    }
    let _ = tls;
    let pid = process::fork(flags, stack, ctid)?;
    if flags & CLONE_PARENT_SETTID != 0 && ptid != 0 {
        write_val(ptid, pid)?;
    }
    Ok(pid as usize)
}

pub fn sys_execve(path: usize, argv: usize, envp: usize) -> SysResult {
    let path = read_string(path, 4096)?;
    let argv = read_str_array(argv, 4096)?;
    let envp = read_str_array(envp, 4096)?;
    let cur = current();
    let cwd = cur.cwd();
    let img = crate::task::exec::load_image(&cwd, &path, argv.clone(), envp, 0)?;
    let name = path.rsplit('/').next().unwrap_or(&path);
    let name = alloc::string::String::from(name);
    crate::task::exec::commit(&cur, img, name);
    Ok(0)
}

fn encode_status(st: i32) -> i32 {
    st
}

pub fn sys_wait4(pid: i32, status: usize, options: i32, _rusage: usize) -> SysResult {
    let (p, st) = process::wait(pid, options)?;
    if p > 0 && status != 0 {
        write_val(status, encode_status(st))?;
    }
    Ok(p as usize)
}

pub fn sys_waitid(idtype: i32, id: i32, infop: usize, options: i32) -> SysResult {
    let pid = match idtype {
        P_ALL => -1,
        P_PID => id,
        P_PGID => -id,
        _ => return Err(EINVAL),
    };
    let opts = if options & WNOHANG != 0 { WNOHANG } else { 0 };
    let (p, st) = process::wait(pid, opts)?;
    if infop != 0 {
        let mut info = SigInfo { si_signo: SIGCHLD, ..SigInfo::default() };
        if p > 0 {
            info.si_pid = p;
            if st & 0x7f != 0 {
                info.si_code = CLD_KILLED;
                info.si_status = st & 0x7f;
            } else {
                info.si_code = CLD_EXITED;
                info.si_status = st >> 8;
            }
        }
        write_val(infop, info)?;
    }
    Ok(0)
}

pub fn sys_getpid() -> SysResult {
    Ok(current().tgid as usize)
}

pub fn sys_gettid() -> SysResult {
    Ok(current().pid as usize)
}

pub fn sys_getppid() -> SysResult {
    Ok(current().ppid() as usize)
}

pub fn sys_getuid() -> SysResult {
    Ok(current().uid.load(Ordering::Relaxed) as usize)
}

pub fn sys_getgid() -> SysResult {
    Ok(current().gid.load(Ordering::Relaxed) as usize)
}

pub fn sys_setuid(uid: u32) -> SysResult {
    if uid != u32::MAX {
        current().uid.store(uid, Ordering::Relaxed);
    }
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> SysResult {
    if gid != u32::MAX {
        current().gid.store(gid, Ordering::Relaxed);
    }
    Ok(0)
}

pub fn sys_getresuid(r: usize, e: usize, s: usize) -> SysResult {
    let uid = current().uid.load(Ordering::Relaxed);
    write_val(r, uid)?;
    write_val(e, uid)?;
    write_val(s, uid)?;
    Ok(0)
}

pub fn sys_getresgid(r: usize, e: usize, s: usize) -> SysResult {
    let gid = current().gid.load(Ordering::Relaxed);
    write_val(r, gid)?;
    write_val(e, gid)?;
    write_val(s, gid)?;
    Ok(0)
}

pub fn sys_setpgid(pid: i32, pgid: i32) -> SysResult {
    let cur = current();
    let target = if pid == 0 { cur.clone() } else { get_task(pid).ok_or(ESRCH)? };
    let pgid = if pgid == 0 { target.pid } else { pgid };
    target.inner.lock().pgid = pgid;
    Ok(0)
}

pub fn sys_getpgid(pid: i32) -> SysResult {
    let cur = current();
    let target = if pid == 0 { cur } else { get_task(pid).ok_or(ESRCH)? };
    let v = target.inner.lock().pgid;
    Ok(v as usize)
}

pub fn sys_getsid(pid: i32) -> SysResult {
    let cur = current();
    let target = if pid == 0 { cur } else { get_task(pid).ok_or(ESRCH)? };
    let v = target.inner.lock().sid;
    Ok(v as usize)
}

pub fn sys_setsid() -> SysResult {
    let cur = current();
    let mut inner = cur.inner.lock();
    inner.sid = cur.pid;
    inner.pgid = cur.pid;
    Ok(cur.pid as usize)
}

pub fn sys_sched_yield() -> SysResult {
    sched::yield_now();
    Ok(0)
}

pub fn sys_sched_getaffinity(_pid: i32, len: usize, mask: usize) -> SysResult {
    if len < 8 {
        return Err(EINVAL);
    }
    write_val(mask, 1u64)?;
    Ok(8)
}

pub fn sys_getrlimit(res: u32, rlim: usize) -> SysResult {
    if res >= 16 {
        return Err(EINVAL);
    }
    let r = current().inner.lock().rlimits[res as usize];
    write_val(rlim, r)?;
    Ok(0)
}

pub fn sys_setrlimit(res: u32, rlim: usize) -> SysResult {
    if res >= 16 {
        return Err(EINVAL);
    }
    let r: Rlimit = read_val(rlim)?;
    let cur = current();
    cur.inner.lock().rlimits[res as usize] = r;
    if res == RLIMIT_NOFILE {
        cur.fds().lock().limit = (r.cur as usize).min(65536);
    }
    Ok(0)
}

pub fn sys_prlimit64(pid: i32, res: u32, new: usize, old: usize) -> SysResult {
    if res >= 16 {
        return Err(EINVAL);
    }
    let task = if pid == 0 { current() } else { get_task(pid).ok_or(ESRCH)? };
    let cur = task.inner.lock().rlimits[res as usize];
    if old != 0 {
        write_val(old, cur)?;
    }
    if new != 0 {
        let r: Rlimit = read_val(new)?;
        task.inner.lock().rlimits[res as usize] = r;
        if res == RLIMIT_NOFILE {
            task.fds().lock().limit = (r.cur as usize).min(65536);
        }
    }
    Ok(0)
}

pub fn sys_getrusage(_who: i32, usage: usize) -> SysResult {
    let cur = current();
    let ut = cur.utime.load(Ordering::Relaxed) as i64;
    let st = cur.stime.load(Ordering::Relaxed) as i64;
    let ru = Rusage {
        ru_utime: Timeval { tv_sec: ut / 1_000_000_000, tv_usec: (ut % 1_000_000_000) / 1000 },
        ru_stime: Timeval { tv_sec: st / 1_000_000_000, tv_usec: (st % 1_000_000_000) / 1000 },
        rest: [0; 14],
    };
    write_val(usage, ru)?;
    Ok(0)
}

pub fn sys_times(buf: usize) -> SysResult {
    let cur = current();
    let ut = cur.utime.load(Ordering::Relaxed) as u64 / 10_000_000; // clock ticks (100 Hz)
    let st = cur.stime.load(Ordering::Relaxed) as u64 / 10_000_000;
    if buf != 0 {
        write_val(buf, [ut, st, 0u64, 0u64])?;
    }
    Ok(crate::time::jiffies() as usize)
}

pub fn sys_prctl(option: i32, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> SysResult {
    match option {
        PR_SET_NAME => {
            let name = read_cstr(arg2, 16).unwrap_or_default();
            let mut name = alloc::string::String::from_utf8_lossy(&name).into_owned();
            name.truncate(15);
            current().inner.lock().name = name;
            Ok(0)
        }
        PR_GET_NAME => {
            let name = current().name();
            let mut buf: Vec<u8> = name.into_bytes();
            buf.truncate(15);
            buf.push(0);
            copy_to_user(arg2, &buf)?;
            Ok(0)
        }
        PR_SET_DUMPABLE | PR_GET_DUMPABLE => Ok(0),
        _ => Ok(0),
    }
}
