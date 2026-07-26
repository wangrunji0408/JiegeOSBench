//! Process management syscalls.

use crate::fs::stat::{RLimit, RUsage, Timeval, RLIM_INFINITY};
use crate::fs::Result;
use crate::mm::uaccess;
use crate::task::{self, futex, CloneFlags, TaskState};
use crate::trap::TrapContext;
use crate::{bail, syscall::SKIP_RETURN};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

pub fn sys_clone(
    cx: &mut TrapContext,
    flags: usize,
    child_stack: usize,
    parent_tid_ptr: usize,
    tls: usize,
    child_tid_ptr: usize,
) -> Result<isize> {
    let clone_flags = CloneFlags::from_bits_truncate(flags & !0xff);
    let task = task::current();

    let Some(child) = task.fork(clone_flags, child_stack) else {
        bail!(ENOMEM);
    };

    // CLONE_SETTLS: the child's `tp` register points at its thread control block.
    if clone_flags.contains(CloneFlags::SETTLS) {
        child.trap_context().set_tls(tls);
    }
    if clone_flags.contains(CloneFlags::CHILD_SETTID) && child_tid_ptr != 0 {
        child.set_child_tid.store(child_tid_ptr, Ordering::Relaxed);
    }
    if clone_flags.contains(CloneFlags::CHILD_CLEARTID) && child_tid_ptr != 0 {
        child.clear_child_tid.store(child_tid_ptr, Ordering::Relaxed);
    }

    let child_tid = child.tid;

    // CLONE_PARENT_SETTID / CHILD_SETTID write the tid before either runs.
    if clone_flags.contains(CloneFlags::PARENT_SETTID) && parent_tid_ptr != 0 {
        uaccess::write(parent_tid_ptr, child_tid as u32)?;
    }
    if clone_flags.contains(CloneFlags::CHILD_SETTID) && child_tid_ptr != 0 {
        // The child shares our address space when CLONE_VM is set, so writing
        // now is equivalent; when it doesn't, the write lands in the COW copy
        // the child will see.
        uaccess::write(child_tid_ptr, child_tid as u32)?;
    }

    let _ = cx;
    task::spawn(child);

    // CLONE_VFORK: the parent must block until the child execs or exits.
    if clone_flags.contains(CloneFlags::VFORK) {
        task::wait_until(|| {
            task::find_task(child_tid)
                .map(|c| c.is_zombie() || c.exe_path() != task.exe_path())
                .unwrap_or(true)
        });
    }

    Ok(child_tid as isize)
}

/// `struct clone_args` for `clone3`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CloneArgs {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

pub fn sys_clone3(cx: &mut TrapContext, args_ptr: usize, size: usize) -> Result<isize> {
    if size < 64 {
        bail!(EINVAL);
    }
    let args: CloneArgs = uaccess::read(args_ptr)?;
    // `clone3` takes the stack base and size; the child's sp goes at the top.
    let child_stack = if args.stack != 0 {
        (args.stack + args.stack_size) as usize
    } else {
        0
    };
    sys_clone(
        cx,
        args.flags as usize,
        child_stack,
        args.parent_tid as usize,
        args.tls as usize,
        args.child_tid as usize,
    )
}

pub fn sys_execve(cx: &mut TrapContext, path_ptr: usize, argv_ptr: usize, envp_ptr: usize) -> isize {
    let result = (|| -> Result<TrapContext> {
        let path = uaccess::read_cstr(path_ptr)?;
        let argv = uaccess::read_cstr_array(argv_ptr)?;
        let envp = uaccess::read_cstr_array(envp_ptr)?;
        crate::loader::exec(&path, &argv, &envp)
    })();

    match result {
        Ok(new_cx) => {
            // Install the new context; the trap return path re-reads it from the
            // task, so we must store it there rather than only editing `cx`.
            task::current().set_trap_context(new_cx);
            SKIP_RETURN
        }
        Err(e) => {
            let _ = cx;
            e.as_ret()
        }
    }
}

pub fn sys_exit(code: i32) -> Result<isize> {
    // The exit status occupies the high byte of the wait status.
    task::exit_current((code & 0xff) << 8);
}

pub fn sys_exit_group(code: i32) -> Result<isize> {
    task::exit_group((code & 0xff) << 8);
}

/// `wait4` options.
const WNOHANG: u32 = 1;
const WUNTRACED: u32 = 2;
const WCONTINUED: u32 = 8;

pub fn sys_wait4(pid: isize, status_ptr: usize, options: u32, rusage_ptr: usize) -> Result<isize> {
    let task = task::current();

    loop {
        // Find a matching child.
        let (found, any_children) = {
            let children = task.group.children.lock();
            let matching: Vec<Arc<task::Task>> = children
                .iter()
                .filter(|c| match pid {
                    // Any child.
                    -1 => true,
                    // Any child in our process group.
                    0 => c.pgid() == task.pgid(),
                    // A specific pid.
                    p if p > 0 => c.pid() == p as usize,
                    // Any child in process group -pid.
                    p => c.pgid() == (-p) as usize,
                })
                .cloned()
                .collect();
            let any = !matching.is_empty();
            let zombie = matching.into_iter().find(|c| c.is_zombie());
            (zombie, any)
        };

        if let Some(child) = found {
            let child_pid = child.pid();
            let exit_code = child.group.exit_code.load(Ordering::Relaxed);

            if status_ptr != 0 {
                uaccess::write(status_ptr, exit_code)?;
            }
            if rusage_ptr != 0 {
                let utime = child.group.utime.load(Ordering::Relaxed) as i64;
                let stime = child.group.stime.load(Ordering::Relaxed) as i64;
                let usage = RUsage {
                    utime: ticks_to_timeval(utime),
                    stime: ticks_to_timeval(stime),
                    ..Default::default()
                };
                uaccess::write(rusage_ptr, usage)?;
            }

            // Reap: drop it from our children list so its memory is released.
            task.group.children.lock().retain(|c| c.pid() != child_pid);
            return Ok(child_pid as isize);
        }

        if !any_children {
            bail!(ECHILD);
        }
        if options & WNOHANG != 0 {
            return Ok(0);
        }

        // Block until a child exits (SIGCHLD wakes us).
        task::yield_now();
        if task::has_pending_signal() {
            // A caught signal interrupts the wait; if it's SIGCHLD the caller
            // will retry.
            bail!(EINTR);
        }
    }
}

fn ticks_to_timeval(ticks: i64) -> Timeval {
    let us = ticks * (1_000_000 / crate::time::TICK_HZ as i64);
    Timeval {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    }
}

pub fn sys_set_tid_address(ptr: usize) -> Result<isize> {
    let task = task::current();
    task.clear_child_tid.store(ptr, Ordering::Relaxed);
    Ok(task.tid as isize)
}

pub fn sys_set_robust_list(head: usize, _len: usize) -> Result<isize> {
    task::current().robust_list.store(head, Ordering::Relaxed);
    Ok(0)
}

pub fn sys_get_robust_list(_tid: i32, head_ptr: usize, len_ptr: usize) -> Result<isize> {
    let head = task::current().robust_list.load(Ordering::Relaxed);
    if head_ptr != 0 {
        uaccess::write(head_ptr, head)?;
    }
    if len_ptr != 0 {
        uaccess::write(len_ptr, 24usize)?;
    }
    Ok(0)
}

pub fn sys_futex(
    uaddr: usize,
    op: u32,
    val: u32,
    timeout_or_val2: usize,
    uaddr2: usize,
    val3: u32,
) -> Result<isize> {
    let cmd = (op as usize) & futex::FUTEX_CMD_MASK;

    match cmd {
        futex::FUTEX_WAIT | futex::FUTEX_WAIT_BITSET => {
            // Re-check the value under no lock but before parking: if it changed,
            // the wakeup already happened.
            let current: u32 = uaccess::read(uaddr)?;
            if current != val {
                bail!(EAGAIN);
            }
            let deadline = if timeout_or_val2 == 0 {
                None
            } else {
                let ts: crate::fs::stat::Timespec = uaccess::read(timeout_or_val2)?;
                let ms = (ts.sec as u64) * 1000 + (ts.nsec as u64) / 1_000_000;
                Some(if cmd == futex::FUTEX_WAIT_BITSET && op as usize & futex::FUTEX_CLOCK_REALTIME != 0 {
                    // An absolute realtime deadline: convert to monotonic ms.
                    let (now_s, now_ns) = crate::time::realtime();
                    let now_ms = now_s * 1000 + now_ns / 1_000_000;
                    crate::time::monotonic_ms() + ms.saturating_sub(now_ms)
                } else {
                    // A relative timeout.
                    crate::time::monotonic_ms() + ms
                })
            };
            let bitset = if cmd == futex::FUTEX_WAIT_BITSET {
                val3
            } else {
                u32::MAX
            };
            futex::wait(uaddr, bitset, deadline)?;
            Ok(0)
        }
        futex::FUTEX_WAKE => Ok(futex::wake(uaddr, val as usize) as isize),
        futex::FUTEX_WAKE_BITSET => Ok(futex::wake_bitset(uaddr, val as usize, val3) as isize),
        futex::FUTEX_REQUEUE => {
            Ok(futex::requeue(uaddr, uaddr2, val as usize, timeout_or_val2) as isize)
        }
        futex::FUTEX_CMP_REQUEUE => {
            let current: u32 = uaccess::read(uaddr)?;
            if current != val3 {
                bail!(EAGAIN);
            }
            Ok(futex::requeue(uaddr, uaddr2, val as usize, timeout_or_val2) as isize)
        }
        _ => {
            crate::warn!("futex: unsupported operation {}", cmd);
            bail!(ENOSYS)
        }
    }
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

pub fn sys_setuid(uid: u32) -> Result<isize> {
    let task = task::current();
    // Only root may change to an arbitrary uid, and once we drop privileges we
    // cannot get them back — which is exactly what nginx's workers expect.
    if task.euid() != 0 && uid != task.uid() && uid != task.euid() {
        bail!(EPERM);
    }
    task.group.uid.store(uid, Ordering::Relaxed);
    task.group.euid.store(uid, Ordering::Relaxed);
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> Result<isize> {
    let task = task::current();
    if task.euid() != 0 && gid != task.gid() && gid != task.egid() {
        bail!(EPERM);
    }
    task.group.gid.store(gid, Ordering::Relaxed);
    task.group.egid.store(gid, Ordering::Relaxed);
    Ok(0)
}

pub fn sys_setreuid(ruid: u32, euid: u32) -> Result<isize> {
    let task = task::current();
    if task.euid() != 0 {
        bail!(EPERM);
    }
    if ruid != u32::MAX {
        task.group.uid.store(ruid, Ordering::Relaxed);
    }
    if euid != u32::MAX {
        task.group.euid.store(euid, Ordering::Relaxed);
    }
    Ok(0)
}

pub fn sys_setregid(rgid: u32, egid: u32) -> Result<isize> {
    let task = task::current();
    if task.euid() != 0 {
        bail!(EPERM);
    }
    if rgid != u32::MAX {
        task.group.gid.store(rgid, Ordering::Relaxed);
    }
    if egid != u32::MAX {
        task.group.egid.store(egid, Ordering::Relaxed);
    }
    Ok(0)
}

pub fn sys_setresuid(ruid: u32, euid: u32, _suid: u32) -> Result<isize> {
    sys_setreuid(ruid, euid)
}

pub fn sys_setresgid(rgid: u32, egid: u32, _sgid: u32) -> Result<isize> {
    sys_setregid(rgid, egid)
}

pub fn sys_getresuid(ruid: usize, euid: usize, suid: usize) -> Result<isize> {
    let task = task::current();
    for (ptr, value) in [
        (ruid, task.uid()),
        (euid, task.euid()),
        (suid, task.euid()),
    ] {
        if ptr != 0 {
            uaccess::write(ptr, value)?;
        }
    }
    Ok(0)
}

pub fn sys_getresgid(rgid: usize, egid: usize, sgid: usize) -> Result<isize> {
    let task = task::current();
    for (ptr, value) in [
        (rgid, task.gid()),
        (egid, task.egid()),
        (sgid, task.egid()),
    ] {
        if ptr != 0 {
            uaccess::write(ptr, value)?;
        }
    }
    Ok(0)
}

pub fn sys_getgroups(size: i32, list_ptr: usize) -> Result<isize> {
    let task = task::current();
    let groups = task.group.groups.lock().clone();
    if size == 0 {
        return Ok(groups.len() as isize);
    }
    if (size as usize) < groups.len() {
        bail!(EINVAL);
    }
    for (i, &g) in groups.iter().enumerate() {
        uaccess::write(list_ptr + i * 4, g)?;
    }
    Ok(groups.len() as isize)
}

pub fn sys_setgroups(size: i32, list_ptr: usize) -> Result<isize> {
    if size < 0 || size > 65536 {
        bail!(EINVAL);
    }
    let task = task::current();
    if task.euid() != 0 {
        bail!(EPERM);
    }
    let mut groups = Vec::with_capacity(size as usize);
    for i in 0..size as usize {
        groups.push(uaccess::read::<u32>(list_ptr + i * 4)?);
    }
    *task.group.groups.lock() = groups;
    Ok(0)
}

pub fn sys_setpgid(pid: usize, pgid: usize) -> Result<isize> {
    let task = task::current();
    let target = if pid == 0 {
        task.clone()
    } else {
        task::find_process(pid).ok_or(crate::err!(ESRCH))?
    };
    let new_pgid = if pgid == 0 { target.pid() } else { pgid };
    target.group.pgid.store(new_pgid, Ordering::Relaxed);
    Ok(0)
}

pub fn sys_getpgid(pid: usize) -> Result<isize> {
    let target = if pid == 0 {
        task::current()
    } else {
        task::find_process(pid).ok_or(crate::err!(ESRCH))?
    };
    Ok(target.pgid() as isize)
}

pub fn sys_setsid() -> Result<isize> {
    let task = task::current();
    // A process group leader cannot create a new session.
    if task.pgid() == task.pid() && task.group.sid.load(Ordering::Relaxed) == task.pid() {
        // Already a session leader; Linux returns EPERM.
        bail!(EPERM);
    }
    let pid = task.pid();
    task.group.sid.store(pid, Ordering::Relaxed);
    task.group.pgid.store(pid, Ordering::Relaxed);
    Ok(pid as isize)
}

pub fn sys_getsid(pid: usize) -> Result<isize> {
    let target = if pid == 0 {
        task::current()
    } else {
        task::find_process(pid).ok_or(crate::err!(ESRCH))?
    };
    Ok(target.group.sid.load(Ordering::Relaxed) as isize)
}

// ---------------------------------------------------------------------------
// prctl
// ---------------------------------------------------------------------------

const PR_SET_PDEATHSIG: u32 = 1;
const PR_GET_PDEATHSIG: u32 = 2;
const PR_SET_DUMPABLE: u32 = 4;
const PR_GET_DUMPABLE: u32 = 3;
const PR_SET_NAME: u32 = 15;
const PR_GET_NAME: u32 = 16;
const PR_SET_NO_NEW_PRIVS: u32 = 38;
const PR_GET_NO_NEW_PRIVS: u32 = 39;
const PR_SET_KEEPCAPS: u32 = 8;
const PR_GET_KEEPCAPS: u32 = 7;

pub fn sys_prctl(op: u32, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> Result<isize> {
    let task = task::current();
    match op {
        PR_SET_NAME => {
            let mut buf = [0u8; 16];
            uaccess::read_into(arg2, &mut buf)?;
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            *task.comm.write() = alloc::string::String::from_utf8_lossy(&buf[..end]).into_owned();
            Ok(0)
        }
        PR_GET_NAME => {
            let name = task.name();
            let mut buf = [0u8; 16];
            let bytes = name.as_bytes();
            let n = bytes.len().min(15);
            buf[..n].copy_from_slice(&bytes[..n]);
            uaccess::write(arg2, buf)?;
            Ok(0)
        }
        PR_SET_DUMPABLE | PR_SET_PDEATHSIG | PR_SET_NO_NEW_PRIVS | PR_SET_KEEPCAPS => Ok(0),
        PR_GET_DUMPABLE => Ok(1),
        PR_GET_PDEATHSIG | PR_GET_NO_NEW_PRIVS | PR_GET_KEEPCAPS => {
            if arg2 != 0 {
                uaccess::write(arg2, 0i32)?;
            }
            Ok(0)
        }
        _ => bail!(EINVAL),
    }
}

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

const RLIMIT_NOFILE: u32 = 7;
const RLIMIT_STACK: u32 = 3;
const RLIMIT_NPROC: u32 = 6;
const RLIMIT_COUNT: u32 = 16;

pub fn sys_getrlimit(resource: u32, ptr: usize) -> Result<isize> {
    if resource >= RLIMIT_COUNT {
        bail!(EINVAL);
    }
    let task = task::current();
    let limit = task.rlimits.lock()[resource as usize];
    uaccess::write(ptr, limit)?;
    Ok(0)
}

pub fn sys_setrlimit(resource: u32, ptr: usize) -> Result<isize> {
    if resource >= RLIMIT_COUNT {
        bail!(EINVAL);
    }
    let limit: RLimit = uaccess::read(ptr)?;
    apply_rlimit(resource, limit)
}

fn apply_rlimit(resource: u32, limit: RLimit) -> Result<isize> {
    let task = task::current();
    if limit.cur > limit.max && limit.max != RLIM_INFINITY {
        bail!(EINVAL);
    }
    task.rlimits.lock()[resource as usize] = limit;

    // RLIMIT_NOFILE actually changes behaviour: nginx raises it via
    // `worker_rlimit_nofile` and then expects to open that many descriptors.
    if resource == RLIMIT_NOFILE {
        let new = if limit.cur == RLIM_INFINITY {
            crate::fs::fdtable::MAX_NOFILE
        } else {
            (limit.cur as usize).min(crate::fs::fdtable::MAX_NOFILE)
        };
        task.files.lock().limit = new;
    }
    Ok(0)
}

pub fn sys_prlimit64(pid: usize, resource: u32, new_ptr: usize, old_ptr: usize) -> Result<isize> {
    if resource >= RLIMIT_COUNT {
        bail!(EINVAL);
    }
    let task = if pid == 0 || pid == task::current().pid() {
        task::current()
    } else {
        task::find_process(pid).ok_or(crate::err!(ESRCH))?
    };

    if old_ptr != 0 {
        let limit = task.rlimits.lock()[resource as usize];
        uaccess::write(old_ptr, limit)?;
    }
    if new_ptr != 0 {
        let limit: RLimit = uaccess::read(new_ptr)?;
        if pid == 0 || pid == task::current().pid() {
            apply_rlimit(resource, limit)?;
        } else {
            task.rlimits.lock()[resource as usize] = limit;
        }
    }
    Ok(0)
}

const RUSAGE_SELF: i32 = 0;
const RUSAGE_CHILDREN: i32 = -1;

pub fn sys_getrusage(who: i32, ptr: usize) -> Result<isize> {
    let task = task::current();
    let (utime, stime) = match who {
        RUSAGE_SELF => (
            task.group.utime.load(Ordering::Relaxed) as i64,
            task.group.stime.load(Ordering::Relaxed) as i64,
        ),
        RUSAGE_CHILDREN => {
            let children = task.group.children.lock();
            let u: i64 = children
                .iter()
                .map(|c| c.group.utime.load(Ordering::Relaxed) as i64)
                .sum();
            let s: i64 = children
                .iter()
                .map(|c| c.group.stime.load(Ordering::Relaxed) as i64)
                .sum();
            (u, s)
        }
        _ => bail!(EINVAL),
    };
    let (used, _) = crate::mm::frame::stats();
    let usage = RUsage {
        utime: ticks_to_timeval(utime),
        stime: ticks_to_timeval(stime),
        maxrss: (used * 4) as i64,
        ..Default::default()
    };
    uaccess::write(ptr, usage)?;
    Ok(0)
}

pub fn sys_umask(mask: u32) -> Result<isize> {
    let task = task::current();
    let old = task.group.umask.swap(mask & 0o777, Ordering::Relaxed);
    Ok(old as isize)
}

pub fn sys_sched_getaffinity(_pid: usize, len: usize, mask_ptr: usize) -> Result<isize> {
    if len < 8 {
        bail!(EINVAL);
    }
    // One CPU, so only bit 0 is set.
    let mut mask = alloc::vec![0u8; len.min(128)];
    mask[0] = 1;
    uaccess::write_bytes(mask_ptr, &mask)?;
    Ok(mask.len() as isize)
}

pub fn sys_sched_getparam(_pid: usize, ptr: usize) -> Result<isize> {
    // `struct sched_param { int sched_priority; }`
    if ptr != 0 {
        uaccess::write(ptr, 0i32)?;
    }
    Ok(0)
}

/// Keep unused wait options documented.
const _: u32 = WUNTRACED | WCONTINUED;
/// Keep the state enum referenced so the import is meaningful.
const _: fn() -> TaskState = || TaskState::Runnable;
/// Keep RLIMIT names documented.
const _: u32 = RLIMIT_STACK | RLIMIT_NPROC;
