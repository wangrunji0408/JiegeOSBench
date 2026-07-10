/// 进程控制块

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::context::{TaskContext, TrapContext};
use crate::config::*;
use crate::mm::{alloc_frame, FrameTracker, MapPerm, MapType, MapArea, MemorySet};

pub type Pid = usize;

/// 文件描述符类型
pub enum FileDesc {
    Stdin,
    Stdout,
    Stderr,
    File {
        inode: Arc<Mutex<crate::fs::ramfs::INode>>,
        offset: usize,
        flags: i32,
    },
    Socket(i32), // socket fd (delegated to net subsystem)
    Pipe { read_end: bool, buf: Arc<Mutex<Vec<u8>>> },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TaskState {
    Ready,
    Running,
    Zombie(i32), // exit code
    Blocking,    // 等待IO
    Sleeping(usize), // 等待到某时间（ms）
}

pub struct Task {
    pub pid: Pid,
    pub ppid: Pid,
    pub state: TaskState,

    /// 内存地址空间
    pub memory_set: MemorySet,

    /// 内核栈（保存任务上下文）
    pub kernel_stack: Vec<u8>,
    pub kernel_stack_top: usize,

    /// 任务切换上下文（保存在内核栈上）
    pub task_context: TaskContext,

    /// 文件描述符表
    pub fds: BTreeMap<i32, FileDesc>,
    pub next_fd: i32,

    /// 当前工作目录
    pub cwd: String,

    /// 程序名
    pub name: String,

    /// 信号掩码等（简化）
    pub exit_code: i32,

    /// 子进程列表
    pub children: Vec<Pid>,

    /// 用户堆顶
    pub brk: usize,

    /// 环境变量
    pub env: Vec<String>,
    pub argv: Vec<String>,

    /// mmaps
    pub mmaps: Vec<MmapRegion>,

    /// epoll table: fd -> (events, data)
    pub epoll_table: alloc::collections::BTreeMap<i32, (u32, u64)>,
}

pub struct MmapRegion {
    pub start: usize,
    pub end: usize,
    pub prot: i32,
    pub flags: i32,
    pub fd: i32,
    pub offset: usize,
}

static NEXT_PID: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(1);

fn alloc_pid() -> Pid {
    NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

impl Task {
    /// 创建init进程（从ELF数据）
    pub fn new_init(elf_data: &[u8]) -> Arc<Mutex<Self>> {
        let pid = alloc_pid();

        // 分配内核栈
        let kernel_stack = alloc::vec![0u8; KERNEL_STACK_SIZE];
        let kernel_stack_top = kernel_stack.as_ptr() as usize + KERNEL_STACK_SIZE;
        println!("[task] New task pid={}, kernel_stack_top={:#x}", pid, kernel_stack_top);

        // 创建内存地址空间并加载ELF（包括动态链接器）
        let elf_result = super::elf::load_elf_full(elf_data, pid);
        let memory_set = elf_result.memory_set;
        let entry = elf_result.entry;
        let user_sp = elf_result.user_sp;
        let brk_start = elf_result.brk_start;

        // 在内核栈上创建初始TrapContext
        let trap_cx = TrapContext::new_user(entry, user_sp);
        let cx_va = kernel_stack_top - 37 * 8;
        unsafe {
            let cx_dst = cx_va as *mut TrapContext;
            cx_dst.write(trap_cx);
            // Set slot 34 (_reserved[0]) to the task's user satp for use by .restore_user
            // This will be written again on first trap entry anyway, but set it correctly here
            let satp_slot = (cx_va + 34 * 8) as *mut usize;
            *satp_slot = memory_set.page_table.token();
        }

        // TaskContext指向__restore，sp指向TrapContext位置
        let task_context = TaskContext::goto_restore(cx_va);

        // 设置文件描述符
        let mut fds = BTreeMap::new();
        fds.insert(0, FileDesc::Stdin);
        fds.insert(1, FileDesc::Stdout);
        fds.insert(2, FileDesc::Stderr);

        let task = Task {
            pid,
            ppid: 0,
            state: TaskState::Ready,
            memory_set,
            kernel_stack,
            kernel_stack_top,
            task_context,
            fds,
            next_fd: 3,
            cwd: String::from("/"),
            name: String::from("nginx"),
            exit_code: 0,
            children: Vec::new(),
            brk: brk_start, // ELF加载后设置
            env: Vec::new(),
            argv: Vec::new(),
            mmaps: Vec::new(),
            epoll_table: alloc::collections::BTreeMap::new(),
        };

        // 激活内存映射
        task.memory_set.activate();

        Arc::new(Mutex::new(task))
    }

    /// Fork创建子进程
    pub fn fork(parent: &Arc<Mutex<Task>>) -> Arc<Mutex<Task>> {
        let pid = alloc_pid();
        let parent_guard = parent.lock();

        // 复制内存空间
        let child_memory = parent_guard.memory_set.clone_for_child();

        // 复制内核栈
        let mut kernel_stack = parent_guard.kernel_stack.clone();
        let kernel_stack_top = kernel_stack.as_ptr() as usize + KERNEL_STACK_SIZE;

        // 复制TrapContext（已在内核栈上）
        let cx_va = kernel_stack_top - 37 * 8;

        // Fix up slot 34 (user satp) to point to CHILD's page table, not parent's
        // Slot 34 = _reserved[0] = offset 34*8 from cx_va
        let child_satp = child_memory.page_table.token();
        unsafe {
            let satp_slot = (cx_va + 34 * 8) as *mut usize;
            *satp_slot = child_satp;
        }

        let task_context = TaskContext::goto_restore(cx_va);

        // 复制文件描述符（简化，共享）
        let fds = parent_guard.fds.iter().map(|(k, v)| {
            let new_fd = match v {
                FileDesc::Stdin => FileDesc::Stdin,
                FileDesc::Stdout => FileDesc::Stdout,
                FileDesc::Stderr => FileDesc::Stderr,
                FileDesc::File { inode, offset, flags } => FileDesc::File {
                    inode: inode.clone(),
                    offset: *offset,
                    flags: *flags,
                },
                FileDesc::Socket(s) => FileDesc::Socket(*s),
                FileDesc::Pipe { read_end, buf } => FileDesc::Pipe {
                    read_end: *read_end,
                    buf: buf.clone(),
                },
            };
            (*k, new_fd)
        }).collect();

        let task = Task {
            pid,
            ppid: parent_guard.pid,
            state: TaskState::Ready,
            memory_set: child_memory,
            kernel_stack,
            kernel_stack_top,
            task_context,
            fds,
            next_fd: parent_guard.next_fd,
            cwd: parent_guard.cwd.clone(),
            name: parent_guard.name.clone(),
            exit_code: 0,
            children: Vec::new(),
            brk: parent_guard.brk,
            env: parent_guard.env.clone(),
            argv: parent_guard.argv.clone(),
            mmaps: Vec::new(),
            epoll_table: parent_guard.epoll_table.clone(),
        };

        Arc::new(Mutex::new(task))
    }

    pub fn alloc_fd(&mut self) -> i32 {
        // 找最小未使用fd
        let mut fd = self.next_fd;
        while self.fds.contains_key(&fd) {
            fd += 1;
        }
        self.next_fd = fd + 1;
        fd
    }

    pub fn get_trap_context(&mut self) -> &mut TrapContext {
        let cx_va = self.kernel_stack_top - 37 * 8;
        unsafe { &mut *(cx_va as *mut TrapContext) }
    }
}

/// 在用户栈上设置argv/envp
fn setup_user_stack(
    memory_set: &mut MemorySet,
    mut sp: usize,
    argv: &[&str],
    envp: &[&str],
) -> (usize, Vec<usize>, Vec<usize>) {
    // 辅助函数：写字符串到栈
    let write_str = |sp: &mut usize, s: &str, memory_set: &mut MemorySet| -> usize {
        let bytes = s.as_bytes();
        *sp -= bytes.len() + 1;
        *sp &= !0; // 不对齐（字节级）
        memory_set.copy_to_user(*sp, bytes);
        memory_set.copy_to_user(*sp + bytes.len(), &[0u8]);
        *sp
    };

    // 写入环境变量字符串
    let mut env_ptrs = Vec::new();
    for e in envp.iter().rev() {
        let ptr = write_str(&mut sp, e, memory_set);
        env_ptrs.push(ptr);
    }
    env_ptrs.reverse();

    // 写入argv字符串
    let mut arg_ptrs = Vec::new();
    for a in argv.iter().rev() {
        let ptr = write_str(&mut sp, a, memory_set);
        arg_ptrs.push(ptr);
    }
    arg_ptrs.reverse();

    // 对齐到8字节
    sp &= !7;

    // auxiliary vector (AT_NULL)
    sp -= 16;
    memory_set.copy_to_user(sp, &[0u8; 16]);

    // NULL终止envp
    sp -= 8;
    memory_set.copy_to_user(sp, &[0u8; 8]);

    // 写入envp指针
    for &ptr in env_ptrs.iter().rev() {
        sp -= 8;
        memory_set.copy_to_user(sp, &(ptr as u64).to_le_bytes());
    }

    // NULL终止argv
    sp -= 8;
    memory_set.copy_to_user(sp, &[0u8; 8]);

    // 写入argv指针
    for &ptr in arg_ptrs.iter().rev() {
        sp -= 8;
        memory_set.copy_to_user(sp, &(ptr as u64).to_le_bytes());
    }

    // argc
    sp -= 8;
    memory_set.copy_to_user(sp, &(argv.len() as u64).to_le_bytes());

    (sp, arg_ptrs, env_ptrs)
}
