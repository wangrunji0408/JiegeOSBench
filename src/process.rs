//! 用户进程：独立地址空间 + 内核栈 + TrapContext + fd 表 + 初始栈。

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::mm::frame::FRAME_ALLOCATOR;
use crate::mm::page_table::{
    PageTable, PTE_R, PTE_W, PTE_X, PTE_U, PTE_A, PTE_D, PTE_G,
};
use crate::mm::address::{PAGE_SIZE, HUGE_PAGE_SIZE};
use crate::mm::{PHYS_RAM_BASE, MEMORY_TOP};
use crate::task::{TaskContext, TaskState, KSTACK_SIZE};
use crate::trap::TrapContext;

pub const USER_STACK_TOP: usize = 0x4000_0000;
pub const USER_STACK_PAGES: usize = 32; // 128KB 用户栈

// Linux auxv 类型
const AT_NULL: usize = 0;
const AT_PHDR: usize = 3;
const AT_PHNUM: usize = 5;
const AT_PAGESZ: usize = 6;
const AT_BASE: usize = 7;
const AT_ENTRY: usize = 9;
const AT_RANDOM: usize = 25;

extern "C" {
    fn __restore();
}

/// 文件描述符表（来自 vfs 模块）
pub type FdTable = crate::vfs::FdTable;

pub struct Process {
    pub pid: usize,
    pub task_ctx: TaskContext,
    pub kstack_top: usize,
    pub trap_ctx_ptr: usize,
    pub root_pa: usize,
    pub state: TaskState,
    pub name: &'static str,
    pub brk: usize,
    pub brk_start: usize,
    pub fd_table: FdTable,
    pub tid_address: usize,
    pub set_child_tid: usize,
    pub next_mmap: usize,
}

impl Process {
    pub fn from_elf(elf: &[u8], pid: usize, name: &'static str) -> Option<Box<Self>> {
        let pt = PageTable::new()?;
        let k_perm = PTE_R | PTE_W | PTE_X | PTE_G; // 无 U
        pt.identity_map_huge_range(PHYS_RAM_BASE, MEMORY_TOP - PHYS_RAM_BASE, k_perm);
        pt.identity_map_huge_range(0x1000_0000, HUGE_PAGE_SIZE, PTE_R | PTE_W | PTE_G);

        let loaded = crate::elf::load_elf(elf, &pt).ok()?;

        // 用户栈：映射若干页
        let mut top_pa = 0usize;
        for i in 0..USER_STACK_PAGES {
            let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
            let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
            pt.map_page(va, pa, PTE_R | PTE_W | PTE_U | PTE_A | PTE_D);
            if i == 0 {
                top_pa = pa;
            }
        }

        // 构造初始栈（argc/argv/envp/auxv）。写物理地址（栈顶页，内核身份映射可直写）
        let phdr_va = loaded.phdr;
        let phnum = loaded.phnum;
        let entry = loaded.entry;
        let user_sp = build_init_stack(top_pa, &["program".to_string()], &[], phdr_va, phnum, 0, entry);

        // 内核栈
        let mut kstack_base = 0usize;
        let pages = KSTACK_SIZE / PAGE_SIZE;
        for i in 0..pages {
            let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
            if i == 0 {
                kstack_base = pa;
            }
        }
        let kstack_top = kstack_base + KSTACK_SIZE;

        let ctx_ptr = (kstack_top - core::mem::size_of::<TrapContext>()) as *mut TrapContext;
        unsafe {
            *ctx_ptr = TrapContext::new_user_entry(entry, user_sp, kstack_top);
        }

        let root_pa = pt.root_pa;
        core::mem::forget(pt);

        let mut fd_table = crate::vfs::FdTable::new();
        // 0/1/2 占位为 stdout/stderr 入口（read/write 时按 fd 特判）
        fd_table.open("/dev/stdin", 0);
        fd_table.open("/dev/stdout", 0);
        fd_table.open("/dev/stderr", 0);

        Some(Box::new(Process {
            pid,
            task_ctx: TaskContext {
                ra: __restore as usize,
                sp: ctx_ptr as usize,
                s: [0; 12],
            },
            kstack_top,
            trap_ctx_ptr: ctx_ptr as usize,
            root_pa,
            state: TaskState::Ready,
            name,
            brk: loaded.brk_start,
            brk_start: loaded.brk_start,
            fd_table,
            tid_address: 0,
            set_child_tid: 0,
            next_mmap: 0x5000_0000,
        }))
    }

    pub fn trap_ctx(&self) -> &'static mut TrapContext {
        unsafe { &mut *(self.trap_ctx_ptr as *mut TrapContext) }
    }

    /// 在进程地址空间映射一页用户可读写内存（用于 mmap/brk），返回虚拟地址
    pub fn map_anon_page(&self, va: usize) -> Option<usize> {
        let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
        let pt = PageTable::from_root(self.root_pa);
        pt.map_page(va, pa, PTE_R | PTE_W | PTE_U | PTE_A | PTE_D);
        Some(va)
    }
}

/// 在用户栈顶构造 argc/argv/envp/auxv，返回最终的 sp（16 字节对齐）。
/// 通过栈顶页的物理地址写入（内核身份映射，无需切 satp）。
fn build_init_stack(
    top_pa: usize,
    args: &[String],
    envp: &[String],
    phdr: usize,
    phnum: usize,
    base: usize,
    entry: usize,
) -> usize {
    // VA -> PA 转换（仅栈顶页）
    let top_page_va = USER_STACK_TOP - PAGE_SIZE;
    let va2pa = |va: usize| -> usize { top_pa + (va - top_page_va) };

    // 随机字节（16 字节）
    let random_bytes: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01,
    ];

    // 1) 字符串区（从栈顶向下）
    let mut p = USER_STACK_TOP;
    let mut arg_str_addrs = Vec::new();
    for a in args {
        let bytes = a.as_bytes();
        p -= bytes.len() + 1;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), va2pa(p) as *mut u8, bytes.len());
            core::ptr::write_volatile(va2pa(p + bytes.len()) as *mut u8, 0);
        }
        arg_str_addrs.push(p);
    }
    let mut env_str_addrs = Vec::new();
    for e in envp {
        let bytes = e.as_bytes();
        p -= bytes.len() + 1;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), va2pa(p) as *mut u8, bytes.len());
            core::ptr::write_volatile(va2pa(p + bytes.len()) as *mut u8, 0);
        }
        env_str_addrs.push(p);
    }
    // 随机字节区
    p -= 16;
    unsafe {
        core::ptr::copy_nonoverlapping(random_bytes.as_ptr(), va2pa(p) as *mut u8, 16);
    }
    let random_addr = p;

    // 2) 计算 need_bytes 并对齐
    let aux_pairs = 7usize;
    let word_count = 1 + args.len() + 1 + envp.len() + 1 + aux_pairs * 2;
    let need_bytes = word_count * 8;
    let mut sp = p - need_bytes;
    sp &= !0xFusize;

    let mut push = |v: usize| {
        sp -= 8;
        unsafe { core::ptr::write_volatile(va2pa(sp) as *mut usize, v); }
    };

    // auxv（逆序压入）
    push(0); push(AT_NULL);
    push(random_addr); push(AT_RANDOM);
    push(entry); push(AT_ENTRY);
    push(base); push(AT_BASE);
    push(phnum); push(AT_PHNUM);
    push(phdr); push(AT_PHDR);
    push(PAGE_SIZE); push(AT_PAGESZ);
    // envp 指针 + NULL
    push(0);
    for &a in env_str_addrs.iter().rev() {
        push(a);
    }
    // argv 指针 + NULL
    push(0);
    for &a in arg_str_addrs.iter().rev() {
        push(a);
    }
    // argc
    push(args.len());

    sp
}
