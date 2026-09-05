//! Process lifecycle: creation, fork, exit, wait.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{alloc_pid, current, sched, signal, SigHandlers, Task, TaskState, PROCESSES};
use crate::abi::*;
use crate::fs::fdtable::FdTable;
use crate::mm::addrspace::AddressSpace;
use crate::sync::SpinLock;

/// First code run by a new task's kernel context: return to user mode.
#[no_mangle]
pub extern "C" fn forkret() -> ! {
    crate::trap::return_to_user()
}

/// Create the init process from `path`.
pub fn spawn_init(path: &str, argv: Vec<Vec<u8>>, envp: Vec<Vec<u8>>) -> Arc<Task> {
    let root = crate::fs::vfs::root();
    let mm = Arc::new(SpinLock::new(AddressSpace::new()));
    let fds = Arc::new(SpinLock::new(FdTable::new()));
    let task = Task::new(
        alloc_pid(),
        String::from("init"),
        mm,
        fds.clone(),
        Arc::new(SpinLock::new(SigHandlers::new())),
        root.clone(),
        alloc::sync::Weak::new(),
    );
    // stdio on the console
    let console = crate::fs::open(&root, "/dev/console", O_RDWR, 0).expect("no /dev/console");
    {
        let mut t = fds.lock();
        t.alloc(console.clone(), false, 0).unwrap();
        t.alloc(console.clone(), false, 0).unwrap();
        t.alloc(console, false, 0).unwrap();
    }
    let img = super::exec::load_image(&root, path, argv, envp, 0).expect("cannot load init");
    // Set the image directly (no current task yet).
    task.set_mm(img.mm);
    task.inner.lock().exe_path = img.path.clone();
    let tf = task.tf();
    *tf = crate::trap::TrapFrame::new_user(img.entry, img.sp, task.kstack_top());
    PROCESSES.lock().insert(task.pid, task.clone());
    sched::make_runnable(&task);
    task
}

/// fork(): duplicate the current task.
pub fn fork(flags: u64, child_stack: usize, child_tid_ptr: usize) -> Result<i32, i32> {
    let parent = current();
    let child_pid = alloc_pid();
    let mm = if flags & CLONE_VM != 0 {
        parent.mm()
    } else {
        let pmm = parent.mm();
        let mut pmm = pmm.lock();
        Arc::new(SpinLock::new(pmm.fork()))
    };
    let fds = if flags & CLONE_FILES != 0 {
        parent.fds()
    } else {
        Arc::new(SpinLock::new(parent.fds().lock().clone_table()))
    };
    let sig = if flags & CLONE_SIGHAND != 0 {
        parent.sig()
    } else {
        let p = parent.sig();
        let p = p.lock();
        Arc::new(SpinLock::new(SigHandlers { actions: p.actions }))
    };
    let (cwd, name, umask, pgid, sid, sigmask, rlimits) = {
        let pi = parent.inner.lock();
        (pi.cwd.clone(), pi.name.clone(), pi.umask, pi.pgid, pi.sid, pi.sigmask, pi.rlimits)
    };
    let child = Task::new(child_pid, name, mm, fds, sig, cwd, Arc::downgrade(&parent));
    {
        let mut ci = child.inner.lock();
        ci.umask = umask;
        ci.pgid = pgid;
        ci.sid = sid;
        ci.sigmask = sigmask;
        ci.rlimits = rlimits;
        ci.exe_path = parent.inner.lock().exe_path.clone();
        if flags & CLONE_CHILD_CLEARTID != 0 {
            ci.clear_child_tid = child_tid_ptr;
        }
    }
    child.uid.store(parent.uid.load(core::sync::atomic::Ordering::Relaxed), core::sync::atomic::Ordering::Relaxed);
    child.gid.store(parent.gid.load(core::sync::atomic::Ordering::Relaxed), core::sync::atomic::Ordering::Relaxed);
    let exit_sig = (flags & 0xff) as i32;
    child.exit_signal.store(exit_sig, core::sync::atomic::Ordering::Relaxed);
    // Copy trap frame; child returns 0.
    let ctf = child.tf();
    *ctf = *parent.tf();
    ctf.kernel_sp = child.kstack_top();
    ctf.set_a0(0);
    if child_stack != 0 {
        ctf.set_sp(child_stack);
    }
    if flags & CLONE_SETTLS != 0 {
        // tp = x4
        ctf.x[4] = parent.tf().x[13]; // a3 holds tls on riscv clone
    }
    if flags & CLONE_CHILD_SETTID != 0 && child_tid_ptr != 0 {
        let _ = crate::mm::uaccess::write_val_mm(&child.mm(), child_tid_ptr, child_pid);
    }
    parent.inner.lock().children.push(child.clone());
    PROCESSES.lock().insert(child_pid, child.clone());
    sched::make_runnable(&child);
    Ok(child_pid)
}

/// Terminate the current task with wait status `status` (already encoded:
/// (code << 8) for normal exit, or the signal number).
pub fn exit_current(status: i32) -> ! {
    let task = current();
    // Close all fds first (drops sockets, wakes peers).
    {
        let fds = task.fds();
        let mut t = fds.lock();
        let all: Vec<Arc<crate::fs::file::File>> = t.iter().map(|(_, e)| e.file.clone()).collect();
        let _ = t;
        drop(all);
        *fds.lock() = FdTable::new();
    }
    // clear_child_tid futex wake
    let ctid = task.inner.lock().clear_child_tid;
    if ctid != 0 {
        let _ = crate::mm::uaccess::write_val(ctid, 0i32);
        crate::syscall::futex::wake(ctid, 1);
    }
    // Reparent children to init.
    let children: Vec<Arc<Task>> = core::mem::take(&mut task.inner.lock().children);
    if !children.is_empty() {
        if let Some(init) = super::get_task(1) {
            for c in &children {
                *c.inner.lock().parent.lock_ref() = Arc::downgrade(&init);
            }
            let mut ii = init.inner.lock();
            for c in children {
                let zombie = c.inner.lock().state == TaskState::Zombie;
                ii.children.push(c);
                if zombie {
                    drop(ii);
                    signal::send_signal(&init, SIGCHLD, None);
                    ii = init.inner.lock();
                }
            }
        }
    }
    let parent = {
        let mut inner = task.inner.lock();
        inner.exit_code = status;
        inner.state = TaskState::Zombie;
        inner.parent.upgrade()
    };
    // Release the address space now (drop our reference).
    task.set_mm(Arc::new(SpinLock::new(AddressSpace::new_empty())));
    klog!("pid {} ({}) exited with status {:#x}", task.pid, task.name(), status);
    if let Some(p) = parent {
        let sig = task.exit_signal.load(core::sync::atomic::Ordering::Relaxed);
        let info = SigInfo {
            si_signo: SIGCHLD,
            si_code: if status & 0x7f != 0 { CLD_KILLED } else { CLD_EXITED },
            si_pid: task.pid,
            si_uid: 0,
            si_status: if status & 0x7f != 0 { status & 0x7f } else { status >> 8 },
            ..SigInfo::default()
        };
        if sig != 0 {
            signal::send_signal(&p, sig, Some(info));
        }
        WAIT_WQ.wake_all();
    } else if task.pid == 1 {
        crate::println!("init exited with status {:#x}; halting", status);
        crate::sbi::shutdown();
    }
    drop(task);
    sched::schedule();
    unreachable!("zombie resumed");
}

pub static WAIT_WQ: crate::task::wait::WaitQueue = crate::task::wait::WaitQueue::new();

/// wait4 implementation. Returns (pid, status).
pub fn wait(pid: i32, options: i32) -> Result<(i32, i32), i32> {
    let cur = current();
    loop {
        // Find a matching zombie child.
        let mut found: Option<Arc<Task>> = None;
        let mut any_match = false;
        {
            let inner = cur.inner.lock();
            for c in &inner.children {
                let matches = if pid == -1 {
                    true
                } else if pid > 0 {
                    c.pid == pid
                } else if pid == 0 {
                    c.inner.lock().pgid == inner.pgid
                } else {
                    c.inner.lock().pgid == -pid
                };
                if !matches {
                    continue;
                }
                any_match = true;
                if c.inner.lock().state == TaskState::Zombie {
                    found = Some(c.clone());
                    break;
                }
            }
        }
        if !any_match {
            return Err(ECHILD);
        }
        if let Some(z) = found {
            let status = z.inner.lock().exit_code;
            // reap
            cur.inner.lock().children.retain(|c| !Arc::ptr_eq(c, &z));
            PROCESSES.lock().remove(&z.pid);
            let zpid = z.pid;
            drop(z);
            return Ok((zpid, status));
        }
        if options & WNOHANG != 0 {
            return Ok((0, 0));
        }
        if signal::has_deliverable(&cur) {
            return Err(EINTR);
        }
        WAIT_WQ.wait();
    }
}
