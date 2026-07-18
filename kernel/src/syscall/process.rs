use crate::fs;
use crate::mm::{translated_str, translated_str_array};
use crate::task::{
    add_task, current_task, current_user_token, exit_current_and_run_next, suspend_current_and_run_next,
    TaskStatus,
};

pub fn sys_exit(exit_code: i32) -> isize {
    exit_current_and_run_next(exit_code);
}

pub fn sys_sched_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid_like() -> isize {
    0
}

pub fn sys_nanosleep(_req: *const u8) -> isize {
    // No real timed sleep queue yet; yielding once is enough to keep
    // callers that merely want to give up the CPU briefly making progress.
    suspend_current_and_run_next();
    0
}

pub fn sys_kill(pid: isize, _sig: i32) -> isize {
    // Real signal delivery is a later milestone; a bare `kill` returning
    // success (without actually delivering anything) is enough for the
    // handful of callers in this workload that just check the return
    // value (e.g. probing whether a pid is alive).
    let _ = pid;
    0
}

pub fn sys_clone(flags: usize, _child_stack: usize, _ptid: usize, _tls: usize, _ctid: usize) -> isize {
    const CSIGNAL_MASK: usize = 0xff;
    if flags & !CSIGNAL_MASK != 0 {
        crate::println!("[kernel] clone with unsupported flags {:#x} (thread support not implemented)", flags);
        return -38; // ENOSYS
    }
    let current = current_task().unwrap();
    let new_task = current.fork();
    let new_pid = new_task.pid();
    new_task.inner_lock().trap_cx().x[10] = 0;
    add_task(new_task);
    new_pid as isize
}

pub fn sys_execve(path: *const u8, argv: *const usize, envp: *const usize) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut args = translated_str_array(token, argv);
    let envs = translated_str_array(token, envp);
    if args.is_empty() {
        args.push(path.clone());
    }
    let data = match fs::open_file(&path, 0) {
        Some(file) => {
            let size = file.size();
            let mut buf = alloc::vec![0u8; size];
            let mut off = 0;
            while off < size {
                let n = file.read_at(off, &mut buf[off..]);
                if n == 0 {
                    break;
                }
                off += n;
            }
            buf
        }
        None => return -2, // ENOENT
    };
    let task = current_task().unwrap();
    task.exec(&data, &args, &envs);
    0
}

pub fn sys_wait4(pid: isize, status_ptr: *mut i32, options: u32) -> isize {
    const WNOHANG: u32 = 1;
    let task = current_task().unwrap();
    loop {
        {
            let mut inner = task.inner_lock();
            if inner.children.is_empty() {
                return -10; // ECHILD
            }
            let idx = inner.children.iter().position(|c| {
                (pid == -1 || c.pid() as isize == pid) && c.inner_lock().task_status == TaskStatus::Zombie
            });
            if let Some(idx) = idx {
                let child = inner.children.remove(idx);
                let found_pid = child.pid();
                let exit_code = child.inner_lock().exit_code;
                drop(inner);
                if !status_ptr.is_null() {
                    let token = current_user_token();
                    let bytes = ((exit_code & 0xff) << 8).to_ne_bytes();
                    let mut chunks = crate::mm::translated_byte_buffer(token, status_ptr as *const u8, 4);
                    let mut copied = 0;
                    for c in chunks.iter_mut() {
                        let n = c.len();
                        c.copy_from_slice(&bytes[copied..copied + n]);
                        copied += n;
                    }
                }
                return found_pid as isize;
            }
            if options & WNOHANG != 0 {
                return 0;
            }
        }
        suspend_current_and_run_next();
    }
}
