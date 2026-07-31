//! Process-related syscalls and init task bootstrap.

use alloc::string::String;
use alloc::vec::Vec;

use crate::console::kprintln;
use crate::task;
use crate::task::TrapFrame;

pub fn sys_exit(code: i32) -> isize {
    task::exit(code)
}

pub fn sys_getpid() -> isize {
    task::current_pid() as isize
}

pub fn sys_getppid() -> isize {
    let t = task::current();
    unsafe { t.as_ref().unwrap().parent.unwrap_or(0) as isize }
}

pub fn sys_clone(flags: usize) -> isize {
    // fork-style clone: SIGCHLD
    if flags & !(0x11 | 0x100) != 0 {
        // CLONE_VM=0x100 (threads) not supported
        if flags & 0x100 != 0 {
            return -38; // ENOSYS for threads
        }
    }
    task::fork()
}

pub fn sys_execve(path: usize, argv: usize, envp: usize) -> isize {
    let path_str = match crate::syscall::read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let argv_ptrs = crate::syscall::read_ptr_array(argv, 256).unwrap_or_default();
    let envp_ptrs = crate::syscall::read_ptr_array(envp, 256).unwrap_or_default();
    let mut argv_strs = Vec::new();
    for p in &argv_ptrs {
        match crate::syscall::read_cstr(*p, 4096) {
            Ok(s) => argv_strs.push(s),
            Err(_) => break,
        }
    }
    let mut envp_strs = Vec::new();
    for p in &envp_ptrs {
        match crate::syscall::read_cstr(*p, 4096) {
            Ok(s) => envp_strs.push(s),
            Err(_) => break,
        }
    }
    exec_into_current(&path_str, &argv_strs, &envp_strs)
}

pub fn exec_into_current(path: &str, argv: &[String], envp: &[String]) -> isize {
    // read file
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    let file_id = match crate::fs::resolve(&cwd, path) {
        Some(id) => id,
        None => return -2, // ENOENT
    };
    let f = match crate::fs::fs().get(file_id) {
        Some(f) => f,
        None => return -2,
    };
    let data = f.borrow().data.clone();
    let execfn = argv.first().cloned().unwrap_or_else(|| path.to_string());

    // load into a fresh mm
    let mut new_mm = crate::mm::vma::Mm::new();
    let res = match crate::elf::load_elf(&mut new_mm, &data, argv, envp, &execfn) {
        Ok(r) => r,
        Err(e) => return e as isize,
    };
    // trampoline
    crate::signal::install_trampoline(&mut new_mm);

    // swap mm in the current task
    let pid = task::current_pid();
    let old_mm = {
        let t = task::task(pid).unwrap();
        let old = core::mem::replace(&mut t.mm, new_mm);
        // reset signals (keep handlers? exec resets handlers to default, keeps mask)
        t.sig.handlers = [crate::signal::SIG_DFL; 64];
        t.sig.flags = [0; 64];
        t.sig.pending = 0;
        t.name = execfn.clone();
        old
    };
    // free old mm (after switching, since we run with new pt)
    let new_pt_root = {
        let t = task::task(pid).unwrap();
        t.mm.pt.root_ppn()
    };
    crate::mm::paging::write_satp(new_pt_root);
    crate::mm::paging::sfence();
    crate::mm::vma::Mm::destroy_raw(old_mm);

    // rebuild the task's trapframe
    let t = task::task(pid).unwrap();
    let kstack_top = t.kstack_top;
    let tf_addr = kstack_top - task::TF_SIZE;
    let ctx_addr = tf_addr - 13 * 8;
    unsafe {
        core::ptr::write_bytes(tf_addr as *mut u8, 0, task::TF_SIZE);
        let tf = tf_addr as *mut TrapFrame;
        (*tf).regs[2] = res.sp;
        (*tf).sepc = res.entry;
        (*tf).sstatus = (1 << 5) | (1 << 18);
        t.tf = tf;
        let ctx = ctx_addr as *mut usize;
        *ctx.add(0) = task::first_run_stub_addr();
        for i in 1..13 {
            *ctx.add(i) = 0;
        }
        t.ctx = ctx_addr;
    }
    0
}

/// Boot: create and start the init task (pid 1) running `path`.
pub fn start_init(path: &str) -> ! {
    let pid = task::new_task();
    // stdio = console
    {
        let t = task::task(pid).unwrap();
        t.name = path.to_string();
        let mut fds = crate::fs::FdTable::new();
        for _ in 0..3 {
            let fd = fds.alloc().unwrap();
            fds.fds[fd] = Some(crate::fs::Fd {
                kind: crate::fs::FdKind::Console,
                flags: 0,
                offset: 0,
                cloexec: false,
                epoll: None,
            });
        }
        t.fds = fds;
        // preload vmas for trampoline
        crate::signal::install_trampoline(&mut t.mm);
    }
    let argv = vec![path.to_string()];
    let envp: Vec<String> = Vec::new();
    let r = exec_into_current(path, &argv, &envp);
    if r != 0 {
        kprintln!("[init] failed to exec {}: {}", path, -r);
        crate::sbi::shutdown();
    }
    task::enter_first_task(pid)
}

pub fn sys_wait4(pid: isize, status_ptr: usize, options: i32, _rusage: usize) -> isize {
    let wnohang = options & 1 != 0;
    loop {
        if let Some((pid, status)) = task::wait4(pid, options) {
            if status_ptr != 0 {
                let _ = crate::syscall::write_user(status_ptr, &(status as u32).to_le_bytes());
            }
            return pid as isize;
        }
        // check ECHILD: no children at all
        let has_children = {
            let cur = task::current_pid();
            unsafe {
                task::TASKS.iter().any(|t| {
                    t.as_ref()
                        .map(|t| t.parent == Some(cur) && t.state != task::TaskState::Zombie)
                        .unwrap_or(false)
                })
            }
        };
        if !has_children && !task::has_zombie() {
            return -10; // ECHILD
        }
        if wnohang {
            return 0;
        }
        // block until a child exits (woken by exit_task)
        let wchan = task::current_pid();
        if crate::signal::has_pending() {
            return -4; // EINTR
        }
        task::block_on(wchan);
    }
}

pub fn sys_waitid(idtype: usize, id: isize, _infop: usize, options: i32) -> isize {
    let pid = if idtype == 0 { id } else { -1 };
    sys_wait4(pid, 0, options, 0)
}

pub fn sys_kill(pid: isize, sig: usize) -> isize {
    if pid <= 0 {
        // signal self (process group semantics simplified)
        crate::signal::send_signal(task::current_pid(), sig);
        return 0;
    }
    if pid as usize == task::current_pid() {
        crate::signal::send_signal(pid as usize, sig);
        return 0;
    }
    crate::signal::send_signal(pid as usize, sig);
    0
}

pub fn sys_getgroups(_size: usize, _list: usize) -> isize {
    0
}

pub fn sys_uname(buf: usize) -> isize {
    // struct utsname: 6 x 65 bytes
    let fields = [
        "Linux",
        "jiegeos",
        "6.6.0-jiege",
        "#1 SMP JiegeOS",
        "riscv64",
        "",
    ];
    let mut data = [0u8; 6 * 65];
    for (i, f) in fields.iter().enumerate() {
        let b = f.as_bytes();
        data[i * 65..i * 65 + b.len()].copy_from_slice(b);
    }
    match crate::syscall::write_user(buf, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_getrlimit(resource: usize, rlim: usize) -> isize {
    // struct rlimit { u64 cur, u64 max }
    let (cur, max) = match resource {
        7 => (1024u64, 1_048_576u64), // RLIMIT_NOFILE
        _ => (1_048_576u64, 1_048_576u64),
    };
    let mut data = [0u8; 16];
    data[..8].copy_from_slice(&cur.to_le_bytes());
    data[8..].copy_from_slice(&max.to_le_bytes());
    match crate::syscall::write_user(rlim, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_prlimit64(pid: isize, resource: usize, new: usize, old: usize) -> isize {
    if pid != 0 && pid as usize != task::current_pid() {
        return -3; // ESRCH
    }
    if old != 0 {
        let r = sys_getrlimit(resource, old);
        if r < 0 {
            return r;
        }
    }
    if new != 0 {
        // accept the new limit (no enforcement)
        let _ = crate::syscall::read_user(new, 16);
    }
    0
}

pub fn sys_getrusage(who: usize, usage: usize) -> isize {
    if usage == 0 {
        return -14;
    }
    let mut data = [0u8; 144];
    let _ = who;
    match crate::syscall::write_user(usage, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_umask(mask: usize) -> isize {
    let t = task::current();
    let old = unsafe {
        let um = &mut t.as_ref().unwrap().sig;
        let _ = um;
        0o22
    };
    let _ = mask;
    old
}

pub fn sys_prctl(option: usize, a1: usize, a2: usize) -> isize {
    match option {
        15 => {
            // PR_SET_NAME
            let t = task::current();
            let name = crate::syscall::read_cstr(a1, 16).unwrap_or_default();
            unsafe {
                t.as_mut().unwrap().name = name;
            }
            0
        }
        16 => {
            // PR_GET_NAME
            let t = task::current();
            let name = unsafe { t.as_ref().unwrap().name.clone() };
            crate::syscall::write_user(a1, name.as_bytes()).unwrap_or(0) as isize
        }
        4 | 8 | 38 => 0, // PR_SET_DUMPABLE, PR_SET_KEEPCAPS, PR_SET_NO_NEW_PRIVS
        _ => {
            let _ = (a1, a2);
            0
        }
    }
}

pub fn sys_gettimeofday(tv: usize, tz: usize) -> isize {
    let _ = tz;
    if tv == 0 {
        return -14;
    }
    let ms = crate::timer::now_ms();
    let sec = ms / 1000;
    let usec = (ms % 1000) * 1000;
    let mut data = [0u8; 16];
    data[..8].copy_from_slice(&(sec as u64).to_le_bytes());
    data[8..].copy_from_slice(&(usec as u64).to_le_bytes());
    match crate::syscall::write_user(tv, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_times(buf: usize) -> isize {
    if buf != 0 {
        let _ = crate::syscall::write_user(buf, &[0u8; 32]);
    }
    (crate::timer::now_ms() * 10) as isize
}

pub fn sys_sysinfo(buf: usize) -> isize {
    if buf == 0 {
        return -14;
    }
    let uptime = (crate::timer::now_ms() / 1000) as u64;
    let totalram = 512u64 * 1024 * 1024;
    let memunit = 1u64;
    let mut data = [0u8; 112];
    data[..8].copy_from_slice(&uptime.to_le_bytes());
    data[8..16].copy_from_slice(&(1u64 << 16).to_le_bytes()); // loads[0]
    data[16..24].copy_from_slice(&(1u64 << 16).to_le_bytes());
    data[24..32].copy_from_slice(&(1u64 << 16).to_le_bytes());
    data[32..40].copy_from_slice(&totalram.to_le_bytes());
    data[40..48].copy_from_slice(&(totalram / 2).to_le_bytes()); // freeram
    data[56..64].copy_from_slice(&memunit.to_le_bytes());
    match crate::syscall::write_user(buf, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_clock_gettime(clock: usize, tp: usize) -> isize {
    if tp == 0 {
        return -14;
    }
    let (sec, nsec) = if clock == 1 {
        // CLOCK_MONOTONIC
        let ms = crate::timer::now_ms();
        (ms / 1000, (ms % 1000) * 1_000_000)
    } else {
        // CLOCK_REALTIME: boot time + uptime (2026-01-01 + uptime)
        let ms = crate::timer::now_ms();
        let boot = 1767225600u64; // 2026-01-01T00:00:00Z approx
        (boot + ms / 1000, (ms % 1000) * 1_000_000)
    };
    let mut data = [0u8; 16];
    data[..8].copy_from_slice(&sec.to_le_bytes());
    data[8..].copy_from_slice(&nsec.to_le_bytes());
    match crate::syscall::write_user(tp, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_clock_getres(_clock: usize, tp: usize) -> isize {
    if tp == 0 {
        return 0;
    }
    let mut data = [0u8; 16];
    data[8..].copy_from_slice(&(10_000_000u64).to_le_bytes()); // 10ms res
    match crate::syscall::write_user(tp, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_nanosleep(req: usize, rem: usize) -> isize {
    let data = match crate::syscall::read_user(req, 16) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let sec = u64::from_le_bytes(data[..8].try_into().unwrap());
    let nsec = u64::from_le_bytes(data[8..].try_into().unwrap());
    let ms = sec * 1000 + nsec / 1_000_000;
    if ms > 0 {
        if crate::signal::has_pending() {
            return -4; // EINTR
        }
        task::sleep(ms);
    }
    if rem != 0 {
        let _ = crate::syscall::write_user(rem, &[0u8; 16]);
    }
    0
}

pub fn sys_clock_nanosleep(clock: usize, flags: usize, req: usize, rem: usize) -> isize {
    let _ = (clock, flags);
    sys_nanosleep(req, rem)
}

pub fn sys_sched_yield() -> isize {
    // yield only if another task is ready
    if !unsafe { task::READY.is_empty() } {
        task::schedule();
    }
    0
}

pub fn sys_sched_getaffinity(pid: usize, cpusetsize: usize, mask: usize) -> isize {
    let _ = pid;
    if cpusetsize == 0 {
        return -22;
    }
    let mut data = vec![0u8; cpusetsize];
    data[0] = 1; // cpu 0
    match crate::syscall::write_user(mask, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

static mut RAND_STATE: u64 = 0x1234_5678_9abc_def0;

pub fn sys_getrandom(buf: usize, len: usize, _flags: usize) -> isize {
    if len == 0 {
        return 0;
    }
    let mut data = vec![0u8; len];
    unsafe {
        for b in data.iter_mut() {
            RAND_STATE = RAND_STATE
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (RAND_STATE >> 33) as u8;
        }
    }
    match crate::syscall::write_user(buf, &data) {
        Ok(_) => len as isize,
        Err(e) => e as isize,
    }
}

pub fn sys_set_tid_address(addr: usize) -> isize {
    let t = task::current();
    unsafe {
        t.as_mut().unwrap().set_tid_address = addr;
    }
    task::current_pid() as isize
}

pub fn sys_futex(addr: usize, op: usize, val: usize, timeout: usize, _val2: usize) -> isize {
    let futex_op = op & 0x7f;
    let clock_realtime = op & (1 << 8) != 0;
    match futex_op {
        0 => {
            // FUTEX_WAIT
            let cur = crate::syscall::read_user(addr, 4).unwrap_or_default();
            let cur = i32::from_le_bytes(cur[..4].try_into().unwrap()) as usize;
            if cur != val {
                return -11; // EAGAIN (value changed)
            }
            if crate::signal::has_pending() {
                return -4; // EINTR
            }
            // block with timeout
            if timeout != 0 {
                let data = crate::syscall::read_user(timeout, 16).unwrap_or_default();
                let sec = u64::from_le_bytes(data[..8].try_into().unwrap());
                let nsec = u64::from_le_bytes(data[8..].try_into().unwrap());
                let ms = sec * 1000 + nsec / 1_000_000;
                let wchan = addr;
                if ms > 0 {
                    crate::timer_wheel::set_timer(
                        crate::timer::now_ms() + ms,
                        wchan,
                        crate::timer_wheel::TimerKind::Wake,
                    );
                }
            }
            task::block_on(addr);
            // woken: check signal
            if crate::signal::has_pending() {
                return -4;
            }
            0
        }
        1 => {
            // FUTEX_WAKE
            let mut n = 0usize;
            for _ in 0..val {
                // wake one task blocked on addr (coarse: wake up to val)
                n += 1;
            }
            crate::task::wake_wchan(addr);
            n as isize
        }
        9 => {
            // FUTEX_WAIT_BITSET
            let _ = clock_realtime;
            sys_futex_inner_wait(addr, val, timeout)
        }
        3 | 4 => {
            // FUTEX_REQUEUE / CMP_REQUEUE: wake up to val
            crate::task::wake_wchan(addr);
            val as isize
        }
        _ => -38,
    }
}

fn sys_futex_inner_wait(addr: usize, val: usize, timeout: usize) -> isize {
    let cur = crate::syscall::read_user(addr, 4).unwrap_or_default();
    let cur = i32::from_le_bytes(cur[..4].try_into().unwrap()) as usize;
    if cur != val {
        return -11;
    }
    if crate::signal::has_pending() {
        return -4;
    }
    if timeout != 0 {
        let data = crate::syscall::read_user(timeout, 16).unwrap_or_default();
        let sec = u64::from_le_bytes(data[..8].try_into().unwrap());
        let nsec = u64::from_le_bytes(data[8..].try_into().unwrap());
        let ms = sec * 1000 + nsec / 1_000_000;
        if ms > 0 {
            crate::timer_wheel::set_timer(
                crate::timer::now_ms() + ms,
                addr,
                crate::timer_wheel::TimerKind::Wake,
            );
        }
    }
    task::block_on(addr);
    if crate::signal::has_pending() {
        return -4;
    }
    0
}
