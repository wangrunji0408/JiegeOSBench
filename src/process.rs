//! 用户进程：独立地址空间 + 内核栈 + TrapContext。

use alloc::boxed::Box;
use crate::mm::frame::FRAME_ALLOCATOR;
use crate::mm::page_table::{
    PageTable, PTE_R, PTE_W, PTE_X, PTE_U, PTE_A, PTE_D, PTE_G,
};
use crate::mm::address::{PAGE_SIZE, HUGE_PAGE_SIZE};
use crate::mm::{PHYS_RAM_BASE, MEMORY_TOP};
use crate::task::{TaskContext, TaskState, KSTACK_SIZE};
use crate::trap::TrapContext;

const USER_STACK_TOP: usize = 0x4000_0000;
const USER_STACK_PAGES: usize = 16; // 64KB 用户栈

extern "C" {
    fn __restore();
}

pub struct Process {
    pub pid: usize,
    pub task_ctx: TaskContext,
    pub kstack_top: usize,
    pub trap_ctx_ptr: usize,
    pub root_pa: usize,
    pub state: TaskState,
    pub name: &'static str,
    pub brk: usize, // 当前 brk 值
    pub brk_start: usize,
}

impl Process {
    /// 从 ELF 字节构造进程（不立即运行，需 sched 入队）
    pub fn from_elf(elf: &[u8], pid: usize, name: &'static str) -> Option<Box<Self>> {
        // 1) 页表：身份映射内核（无 U 位）+ 加载用户段（U 位）
        let pt = PageTable::new()?;
        let k_perm = PTE_R | PTE_W | PTE_X | PTE_G; // 无 U：U-mode 不可访问，S-mode 可
        pt.identity_map_huge_range(PHYS_RAM_BASE, MEMORY_TOP - PHYS_RAM_BASE, k_perm);
        pt.identity_map_huge_range(0x1000_0000, HUGE_PAGE_SIZE, PTE_R | PTE_W | PTE_G);

        let loaded = crate::elf::load_elf(elf, &pt).ok()?;

        // 2) 用户栈
        for i in 0..USER_STACK_PAGES {
            let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
            let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
            pt.map_page(va, pa, PTE_R | PTE_W | PTE_U | PTE_A | PTE_D);
        }

        // 3) 内核栈
        let mut kstack_base = 0usize;
        let pages = KSTACK_SIZE / PAGE_SIZE;
        for i in 0..pages {
            let pa = FRAME_ALLOCATOR.alloc_zeroed()?;
            if i == 0 {
                kstack_base = pa;
            }
        }
        let kstack_top = kstack_base + KSTACK_SIZE;

        // 4) TrapContext 放在内核栈顶
        let ctx_ptr = (kstack_top - core::mem::size_of::<TrapContext>()) as *mut TrapContext;
        let user_sp = USER_STACK_TOP; // 16 字节对齐
        unsafe {
            *ctx_ptr = TrapContext::new_user_entry(loaded.entry, user_sp, kstack_top);
        }

        let root_pa = pt.root_pa;
        core::mem::forget(pt); // 保留页表帧，不释放

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
        }))
    }

    pub fn trap_ctx(&self) -> &'static mut TrapContext {
        unsafe { &mut *(self.trap_ctx_ptr as *mut TrapContext) }
    }
}
