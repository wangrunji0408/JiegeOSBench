//! 用户进程：独立地址空间 + 内核栈 + TrapContext + fd 表 + 初始栈。

use alloc::boxed::Box;
use alloc::string::String;
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

/// 文件描述符表项
#[derive(Clone)]
pub struct FdEntry {
    pub kind: FdKind,
}

#[derive(Clone, PartialEq)]
pub enum FdKind {
    Stdio, // 0/1/2
}

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
    pub fd_table: Vec<Option<FdEntry>>,
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
        for i in 0..USER_STACK_PAGES {
            let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
            let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
            pt.map_page(va, pa, PTE_R | PTE_W | PTE_U | PTE_A | PTE_D);
        }

        // 构造初始栈（argc/argv/envp/auxv）
        let phdr_va = loaded.phdr;
        let phnum = loaded.phnum;
        let entry = loaded.entry;
        let user_sp = build_init_stack(&pt, &["program".to_string()], &[], phdr_va, phnum, 0, entry);

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

        let mut fd_table: Vec<Option<FdEntry>> = Vec::new();
        fd_table.resize_with(16, || None);
        fd_table[0] = Some(FdEntry { kind: FdKind::Stdio });
        fd_table[1] = Some(FdEntry { kind: FdKind::Stdio });
        fd_table[2] = Some(FdEntry { kind: FdKind::Stdio });

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
fn build_init_stack(
    pt: &PageTable,
    args: &[String],
    envp: &[String],
    phdr: usize,
    phnum: usize,
    base: usize,
    entry: usize,
) -> usize {
    // 栈顶向下写。先准备所有字符串，再准备指针/数值区。
    let top = USER_STACK_TOP;
    // 随机字节（16 字节）
    let random_bytes: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
        0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01,
    ];

    // 1) 字符串区
    let mut p = top;
    let mut arg_str_addrs = Vec::new();
    for a in args {
        let bytes = a.as_bytes();
        p -= bytes.len() + 1;
        write_user_bytes(pt, p, bytes);
        write_user_byte(pt, p + bytes.len(), 0);
        arg_str_addrs.push(p);
    }
    let mut env_str_addrs = Vec::new();
    for e in envp {
        let bytes = e.as_bytes();
        p -= bytes.len() + 1;
        write_user_bytes(pt, p, bytes);
        write_user_byte(pt, p + bytes.len(), 0);
        env_str_addrs.push(p);
    }
    // 随机字节区
    p -= 16;
    write_user_bytes(pt, p, &random_bytes);
    let random_addr = p;

    // 2) 对齐到 16（sp 最终需 16 对齐）。先计算后续需要的字数
    // auxv: AT_PAGESZ, AT_PHDR, AT_PHNUM, AT_BASE, AT_ENTRY, AT_RANDOM, AT_NULL = 7 对 = 14 字
    // null term: envp NULL (1) + argv NULL (1) = 2 字
    let aux_pairs = 7;
    let word_count = 1 /*argc*/ + args.len() + 1 + envp.len() + 1 + aux_pairs * 2;
    let need_bytes = word_count * 8;
    // 对齐 p 到 16，并预留 need_bytes
    let mut sp = p - need_bytes;
    sp &= !0xFusize;

    let mut push = |v: usize| {
        sp -= 8;
        write_user_word(pt, sp, v);
    };

    // auxv（逆序压入，这样正序读出）
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

fn write_user_bytes(pt: &PageTable, va: usize, data: &[u8]) {
    // SUM 已设，内核可写用户页
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len());
    }
}
fn write_user_byte(pt: &PageTable, va: usize, b: u8) {
    unsafe { core::ptr::write_volatile(va as *mut u8, b); }
}
fn write_user_word(pt: &PageTable, va: usize, w: usize) {
    unsafe { core::ptr::write_volatile(va as *mut usize, w); }
}
