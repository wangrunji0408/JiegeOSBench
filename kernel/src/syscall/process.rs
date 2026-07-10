/// 进程管理相关syscall

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::arch::context::TrapContext;
use crate::task::{current_task, manager::TASK_MANAGER, add_task};
use crate::task::process::{Task, TaskState, FileDesc};

use super::*;

pub fn sys_exit(code: i32) -> isize {
    println!("[syscall] Process exit with code {}", code);
    crate::task::current_task_exit(code);
}

pub fn sys_getpid() -> isize {
    current_task()
        .map(|t| t.lock().pid as isize)
        .unwrap_or(1)
}

pub fn sys_getppid() -> isize {
    current_task()
        .map(|t| t.lock().ppid as isize)
        .unwrap_or(0)
}

pub fn sys_clone(
    flags: usize,
    stack: usize,
    ptid: usize,
    tls: usize,
    ctid: usize,
    cx: &mut TrapContext,
) -> isize {
    const SIGCHLD: usize = 17;
    const CLONE_VFORK: usize = 0x4000;
    const CLONE_VM: usize = 0x0100;
    const CLONE_FS: usize = 0x0200;
    const CLONE_FILES: usize = 0x0400;
    const CLONE_THREAD: usize = 0x10000;

    // 简化：只支持fork（flags=SIGCHLD）
    println!("[fork] clone flags={:#x}", flags);
    let task = current_task().unwrap();
    let child = Task::fork(&task);

    {
        let mut child = child.lock();
        // 子进程的返回值是0
        child.get_trap_context().set_a0(0);
        // 如果指定了stack，更新子进程的sp
        if stack != 0 {
            child.get_trap_context().x[2] = stack;
        }
        let child_pid = child.pid;

        // 父进程的children列表
        task.lock().children.push(child_pid);

        // 如果有ptid，写入子进程pid
        if ptid != 0 {
            task.lock().memory_set.copy_to_user(ptid, &(child_pid as u32).to_le_bytes());
        }
    }

    let child_pid = child.lock().pid;
    add_task(child);

    child_pid as isize // 父进程返回子进程pid
}

pub fn sys_execve(
    path_va: usize,
    argv_va: usize,
    envp_va: usize,
    cx: &mut TrapContext,
) -> isize {
    let task = current_task().unwrap();
    let (path, argv, envp) = {
        let t = task.lock();
        let path = t.memory_set.page_table.read_cstr(path_va);

        // 读取argv
        let mut argv = Vec::new();
        let mut ptr_va = argv_va;
        loop {
            let mut ptr_buf = [0u8; 8];
            t.memory_set.copy_from_user(ptr_va, &mut ptr_buf);
            let ptr = usize::from_le_bytes(ptr_buf);
            if ptr == 0 { break; }
            argv.push(t.memory_set.page_table.read_cstr(ptr));
            ptr_va += 8;
        }

        // 读取envp
        let mut envp = Vec::new();
        let mut ptr_va = envp_va;
        loop {
            let mut ptr_buf = [0u8; 8];
            t.memory_set.copy_from_user(ptr_va, &mut ptr_buf);
            let ptr = usize::from_le_bytes(ptr_buf);
            if ptr == 0 { break; }
            envp.push(t.memory_set.page_table.read_cstr(ptr));
            ptr_va += 8;
        }

        (path, argv, envp)
    };

    println!("[syscall] execve: {}", path);

    // 从文件系统加载ELF
    let elf_data = match crate::fs::FS.lookup(&path) {
        Some(node) => {
            let node = node.lock();
            match &node.kind {
                crate::fs::ramfs::INodeKind::File(data) => {
                    let d = data.lock();
                    if d.is_empty() {
                        println!("[syscall] execve: empty file {}", path);
                        return ENOEXEC;
                    }
                    d.clone()
                }
                _ => return EACCES,
            }
        }
        None => {
            println!("[syscall] execve: not found {}", path);
            return ENOENT;
        }
    };

    // 重建内存地址空间
    let pid = task.lock().pid;
    let argv_strs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let envp_strs: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    let (new_memory, entry, user_sp) = crate::task::elf::load_elf(&elf_data, pid);

    // 设置栈
    let mut new_memory = new_memory;
    // TODO: 正确设置argv/envp在栈上

    {
        let mut t = task.lock();
        t.memory_set = new_memory;
        t.name = path.clone();
        t.brk = 0;
        t.mmaps.clear();
        // 保留fd 0,1,2（stdin/stdout/stderr），关闭其他
        t.fds.retain(|&fd, _| fd <= 2);

        // 更新TrapContext
        let trap_cx = TrapContext::new_user(entry, user_sp);
        *t.get_trap_context() = trap_cx;

        t.memory_set.activate();
    }

    // syscall返回值在TrapContext中设置了
    0
}

pub fn sys_wait4(pid: i32, status_va: usize, options: i32) -> isize {
    const WNOHANG: i32 = 1;

    loop {
        let found_option: Option<(usize, i32)> = {
            let task = current_task().unwrap();
            let t = task.lock();
            let children = t.children.clone();
            drop(t);

            let mut found = None;
            for cpid in &children {
                let mgr = TASK_MANAGER.lock();
                if let Some(child) = mgr.tasks.get(cpid) {
                    let child = child.lock();
                    match child.state {
                        TaskState::Zombie(code) => {
                            found = Some((*cpid, code));
                            break;
                        }
                        _ => {}
                    }
                }
            }
            found
        };

        if let Some((cpid, code)) = found_option {
            // 子进程已退出
            if status_va != 0 {
                let status = (code as u32) << 8; // WEXITSTATUS
                let task = current_task().unwrap();
                let t = task.lock();
                t.memory_set.copy_to_user(status_va, &status.to_le_bytes());
            }
            // 从任务管理器移除
            TASK_MANAGER.lock().tasks.remove(&cpid);
            // 从父进程children列表移除
            let task = current_task().unwrap();
            task.lock().children.retain(|&p| p != cpid);
            return cpid as isize;
        }

        if options & WNOHANG != 0 {
            return 0;
        }

        // 等待
        crate::task::schedule();
    }
}

pub fn sys_uname(buf_va: usize) -> isize {
    #[repr(C)]
    struct Utsname {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    let mut uts: Utsname = unsafe { core::mem::zeroed() };
    copy_str(&mut uts.sysname, "Linux");
    copy_str(&mut uts.nodename, "jiege-os");
    copy_str(&mut uts.release, "5.15.0");
    copy_str(&mut uts.version, "#1 SMP JiegeOS");
    copy_str(&mut uts.machine, "riscv64");
    copy_str(&mut uts.domainname, "(none)");

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(buf_va, bytemuck_cast(core::slice::from_ref(&uts)));
    0
}

fn copy_str(dst: &mut [u8], src: &str) {
    let bytes = src.as_bytes();
    let n = bytes.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&bytes[..n]);
    dst[n] = 0;
}

fn bytemuck_cast<T>(s: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(
            s.as_ptr() as *const u8,
            s.len() * core::mem::size_of::<T>(),
        )
    }
}

#[repr(C)]
struct Rlimit {
    rlim_cur: u64,
    rlim_max: u64,
}

pub fn sys_getrlimit(resource: i32, rlimit_va: usize) -> isize {
    const RLIM_INFINITY: u64 = u64::MAX;
    const RLIMIT_NOFILE: i32 = 7;
    const RLIMIT_STACK: i32 = 3;

    let rlimit = match resource {
        RLIMIT_NOFILE => Rlimit { rlim_cur: 1024, rlim_max: 4096 },
        RLIMIT_STACK => Rlimit { rlim_cur: 8 * 1024 * 1024, rlim_max: RLIM_INFINITY },
        _ => Rlimit { rlim_cur: RLIM_INFINITY, rlim_max: RLIM_INFINITY },
    };

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(rlimit_va, bytemuck_cast(core::slice::from_ref(&rlimit)));
    0
}

pub fn sys_prlimit64(pid: i32, resource: i32, new_limit_va: usize, old_limit_va: usize) -> isize {
    if old_limit_va != 0 {
        sys_getrlimit(resource, old_limit_va);
    }
    0
}

pub fn sys_kill(pid: i32, sig: i32) -> isize {
    if sig == 0 { return 0; } // 只检查进程是否存在
    // 简化：忽略信号
    0
}

pub fn sys_rt_sigprocmask(how: i32, set_va: usize, old_set_va: usize, size: usize) -> isize {
    if old_set_va != 0 {
        let task = current_task().unwrap();
        let t = task.lock();
        let empty = [0u64; 1];
        t.memory_set.copy_to_user(old_set_va, bytemuck_cast(&empty));
    }
    0
}

pub fn sys_rt_sigaction(sig: i32, act_va: usize, old_act_va: usize, size: usize) -> isize {
    if old_act_va != 0 {
        let task = current_task().unwrap();
        let t = task.lock();
        let empty = [0u8; 32];
        t.memory_set.copy_to_user(old_act_va, &empty);
    }
    0
}

pub fn sys_futex(uaddr: usize, op: i32, val: u32, timeout: usize, uaddr2: usize, val3: u32) -> isize {
    const FUTEX_WAIT: i32 = 0;
    const FUTEX_WAKE: i32 = 1;
    const FUTEX_PRIVATE_FLAG: i32 = 128;

    let op = op & !FUTEX_PRIVATE_FLAG;
    match op {
        FUTEX_WAIT => {
            // 检查uaddr的值是否还是val
            let task = current_task().unwrap();
            let t = task.lock();
            let mut cur_val_buf = [0u8; 4];
            t.memory_set.copy_from_user(uaddr, &mut cur_val_buf);
            let cur_val = u32::from_le_bytes(cur_val_buf);
            if cur_val != val {
                return EAGAIN;
            }
            drop(t);
            // 放弃CPU，等待唤醒
            let pid = current_task().unwrap().lock().pid;
            {
                let mut mgr = TASK_MANAGER.lock();
                if let Some(task) = mgr.tasks.get(&pid) {
                    task.lock().state = TaskState::Blocking;
                }
            }
            crate::task::schedule();
            0
        }
        FUTEX_WAKE => {
            // 唤醒等待的任务（简化：唤醒所有blocking任务）
            TASK_MANAGER.lock().wake_io_tasks();
            1
        }
        _ => ENOSYS,
    }
}

pub fn sys_getrusage(who: i32, rusage_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();
    let empty = [0u8; 144]; // struct rusage
    t.memory_set.copy_to_user(rusage_va, &empty);
    0
}

pub fn sys_sysinfo(info_va: usize) -> isize {
    #[repr(C)]
    struct SysInfo {
        uptime: i64,
        loads: [u64; 3],
        totalram: u64,
        freeram: u64,
        sharedram: u64,
        bufferram: u64,
        totalswap: u64,
        freeswap: u64,
        procs: u16,
        _pad1: [u8; 6],
        totalhigh: u64,
        freehigh: u64,
        mem_unit: u32,
        _pad2: [u8; 0],
    }

    let info = SysInfo {
        uptime: (crate::timer::get_time_ms() / 1000) as i64,
        loads: [0; 3],
        totalram: 128 * 1024 * 1024,
        freeram: 64 * 1024 * 1024,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: 1,
        _pad1: [0; 6],
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        _pad2: [],
    };

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(info_va, bytemuck_cast(core::slice::from_ref(&info)));
    0
}

pub fn sys_sched_getaffinity(pid: i32, cpusetsize: usize, mask_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();
    // 单核系统，CPU 0
    let mask = [1u8];
    let mut buf = vec![0u8; cpusetsize];
    buf[0] = 1; // CPU 0
    t.memory_set.copy_to_user(mask_va, &buf);
    0
}

pub fn sys_sched_getparam(pid: i32, param_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();
    let param = [0i32; 1]; // sched_priority = 0
    t.memory_set.copy_to_user(param_va, bytemuck_cast(&param));
    0
}

// 共享内存实现（简化版）
use alloc::collections::BTreeMap;
use spin::Mutex;
use lazy_static::lazy_static;

struct ShmSegment {
    data: Vec<u8>,
    size: usize,
}

lazy_static! {
    static ref SHM_TABLE: Mutex<BTreeMap<i32, alloc::sync::Arc<Mutex<ShmSegment>>>> =
        Mutex::new(BTreeMap::new());
    static ref NEXT_SHM_ID: Mutex<i32> = Mutex::new(1);
}

pub fn sys_shmget(key: usize, size: usize, flags: i32) -> isize {
    let mut next = NEXT_SHM_ID.lock();
    let id = *next;
    *next += 1;

    let seg = ShmSegment {
        data: vec![0u8; size],
        size,
    };
    SHM_TABLE.lock().insert(id, alloc::sync::Arc::new(Mutex::new(seg)));
    id as isize
}

pub fn sys_shmat(shmid: i32, shmaddr: usize, shmflg: i32) -> isize {
    // 把共享内存映射到用户空间
    let seg = match SHM_TABLE.lock().get(&shmid).cloned() {
        Some(s) => s,
        None => return super::EINVAL,
    };

    let size = seg.lock().size;
    let task = current_task().unwrap();
    let mut t = task.lock();

    // 找一个空闲地址
    let addr = if shmaddr == 0 {
        crate::syscall::mm::find_free_mmap_addr_pub(&t, size)
    } else {
        shmaddr
    };

    let map_end = (addr + size + crate::config::PAGE_SIZE - 1) & !(crate::config::PAGE_SIZE - 1);

    // 映射内存
    let mut area = crate::mm::MapArea::new(
        addr, map_end,
        crate::mm::MapType::Framed,
        crate::mm::MapPerm::R | crate::mm::MapPerm::W | crate::mm::MapPerm::U,
    );

    // 逐页映射
    let mut frames = alloc::collections::BTreeMap::new();
    for vpn in (addr >> crate::config::PAGE_SIZE_BITS)..(map_end >> crate::config::PAGE_SIZE_BITS) {
        let frame = crate::mm::alloc_frame().expect("no mem");
        let ppn = frame.ppn();
        t.memory_set.page_table.remap(vpn, ppn, crate::mm::PTEFlags::from(crate::mm::MapPerm::R | crate::mm::MapPerm::W | crate::mm::MapPerm::U));
        frames.insert(vpn, frame);
    }
    area.frames = frames;
    t.memory_set.areas.push(area);

    addr as isize
}

