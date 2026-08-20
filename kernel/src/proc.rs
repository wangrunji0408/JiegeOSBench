//! 用户进程：单进程模型（无调度器）
//!
//! nginx 以 master_process off 单进程模式运行，内核一次只运行一个用户进程，
//! IO 等待在 syscall 内部通过 poll 循环 + wfi 完成。

use alloc::string::String;
use alloc::vec::Vec;
use crate::errno::{Errno, Ret};
use crate::page::{self, PTE_A, PTE_D, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X};
use crate::pmm;
use crate::pmm::spin::Mutex;
use crate::{kprintln, net, vfs};

pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub flags: u64, // PTE R/W/X/U
}

pub struct Process {
    pub pid: usize,
    pub root: usize, // 用户页表 root 物理地址
    pub vmas: Vec<Vma>,
    pub brk: usize,
    pub mmap_next: usize,
    pub cwd: String,
    pub fds: Vec<Option<crate::net::socket::FdEntry>>,
    pub exiting: bool,
}

static mut PROCESS: Option<Process> = None;

pub fn current() -> &'static mut Process {
    unsafe {
        #[allow(static_mut_refs)]
        PROCESS.as_mut().expect("no process")
    }
}

pub fn current_page_table_root() -> usize {
    current().root
}

/// 在用户页表中映射一个零页（lazy fault 用）
pub fn map_zero_page(va: usize) -> bool {
    let proc = current();
    let va = va & !0xfff;
    // 必须落在某个 VMA 内
    if !proc.vmas.iter().any(|v| va >= v.start && va < v.end) {
        return false;
    }
    let vma = proc.vmas.iter().find(|v| va >= v.start && va < v.end).unwrap();
    if let Some(pa) = pmm::alloc_page() {
        page::map_4k(proc.root, va, pa, vma.flags | PTE_A | PTE_D | PTE_V);
        true
    } else {
        false
    }
}

/// 处理用户态 page fault
pub fn handle_page_fault(va: usize, _code: u64) -> Result<(), i32> {
    if map_zero_page(va) {
        Ok(())
    } else {
        Err(crate::errno::SIGSEGV)
    }
}

/// 申请一段用户地址区间（bump 分配）
pub fn alloc_user_range(len: usize, flags: u64) -> Option<(usize, usize)> {
    let proc = current();
    let start = proc.mmap_next;
    let end = (start + len + 0xfff) & !0xfff;
    proc.mmap_next = end;
    proc.vmas.push(Vma { start, end, flags });
    Some((start, end))
}

/// 在指定地址建立 VMA（ELF 加载用），失败返回 None
pub fn add_vma(start: usize, end: usize, flags: u64) -> bool {
    let proc = current();
    let start = start & !0xfff;
    let end = (end + 0xfff) & !0xfff;
    if start >= end {
        return false;
    }
    // 禁止用户 VMA 进入内核恒等映射区（MMIO 0x10000000 附近 + RAM 0x80000000+）
    if start < 0x8800_0000 && end > 0x0800_0000 {
        return false;
    }
    // 与既有 VMA 重叠检查
    for v in proc.vmas.iter() {
        if start < v.end && end > v.start {
            return false;
        }
    }
    proc.vmas.push(Vma { start, end, flags });
    true
}

/// 分配一个 fd
pub fn alloc_fd() -> usize {
    let proc = current();
    for (i, f) in proc.fds.iter().enumerate() {
        if f.is_none() {
            return i;
        }
    }
    proc.fds.push(None);
    proc.fds.len() - 1
}

pub fn get_fd(fd: usize) -> Option<&'static mut crate::net::socket::FdEntry> {
    let proc = current();
    if fd >= proc.fds.len() {
        return None;
    }
    // PROCESS 是 static mut，借用可以视作 'static（单线程访问）
    proc.fds[fd]
        .as_mut()
        .map(|e| unsafe { core::mem::transmute::<&mut _, &'static mut _>(e) })
}

/// 进程死亡：打印并关机
pub fn die(sig: i32) -> ! {
    kprintln!("[process] killed by signal {}", sig);
    net::poll_flush();
    crate::sbi::shutdown()
}

/// 启动第一个用户进程（execve 语义）
pub fn spawn(argv: &[&str]) -> Result<(), Errno> {
    let path = argv[0];
    let (file_data, _mode) = vfs::open_read(path).ok_or(Errno::Enoent)?;
    let data: alloc::vec::Vec<u8> = match file_data {
        vfs::FileData::Static(b) => b.to_vec(),
        vfs::FileData::Tmp(v) => v.borrow().clone(),
    };

    // 创建用户页表
    let root = page::alloc_table().ok_or(Errno::Enomem)?;
    page::map_kernel_regions(root);

    // 初始化进程结构
    unsafe {
        #[allow(static_mut_refs)]
        {
            PROCESS = Some(Process {
                pid: 1,
                root,
                vmas: Vec::new(),
                brk: 0,
                mmap_next: 0x3000_0000_00,
                cwd: String::from("/"),
                fds: Vec::new(),
                exiting: false,
            });
        }
    }

    let interp_and_entry = crate::elf::load_program(data, argv);
    if let Err(e) = interp_and_entry {
        kprintln!("[spawn] failed to load {}: {:?}", path, e);
        return Err(e);
    }

    // 标准 IO: 0/1/2 → console
    let proc = current();
    for _ in 0..3 {
        let fd = proc.fds.len();
        proc.fds.push(Some(crate::net::socket::FdEntry::console()));
        let _ = fd;
    }

    // 进入用户态
    let frame = crate::trap::TRAPFRAME_ADDR as *mut crate::trap::TrapFrame;
    crate::trap::enter_user(frame)
}
