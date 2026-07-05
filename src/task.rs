//! 任务（内核线程）抽象与上下文切换。

use core::arch::global_asm;
use alloc::boxed::Box;
use crate::mm::frame::FRAME_ALLOCATOR;
use crate::mm::PAGE_SIZE;
use crate::trap::TrapContext;

global_asm!(include_str!("switch.S"));

const KSTACK_PAGES: usize = 4; // 16KB 内核栈
pub const KSTACK_SIZE: usize = PAGE_SIZE * KSTACK_PAGES;

/// 任务上下文，与 switch.S 布局对应：ra, sp, s0..s11
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskContext {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

impl TaskContext {
    pub const fn zero() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
pub enum TaskState {
    Ready,
    Running,
    Exited,
}

pub struct Task {
    pub id: usize,
    pub task_ctx: TaskContext,
    pub kstack_top: usize, // 内核栈顶物理地址
    pub state: TaskState,
    pub name: &'static str,
}

extern "C" {
    fn __restore();
    fn __switch(cur: *mut TaskContext, next: *const TaskContext);
}

impl Task {
    /// 创建一个内核任务，入口为 entry。首次调度时从 __restore 进入。
    pub fn new_kernel(id: usize, entry: usize, name: &'static str) -> Box<Self> {
        // 分配内核栈
        let mut kstack_base = 0usize;
        for i in 0..KSTACK_PAGES {
            let pa = FRAME_ALLOCATOR
                .alloc_zeroed()
                .expect("OOM: kernel stack");
            if i == 0 {
                kstack_base = pa;
            }
        }
        let kstack_top = kstack_base + KSTACK_SIZE;

        // 在栈顶放置初始 TrapContext
        let ctx_ptr = (kstack_top - core::mem::size_of::<TrapContext>()) as *mut TrapContext;
        unsafe {
            let ctx = &mut *ctx_ptr;
            // 清零
            ctx.x = [0; 32];
            ctx.sepc = entry;
            ctx.sstatus = (1 << 8) | (1 << 5); // SPP=1(S-mode), SPIE=1(开中断后)
            ctx.stval = 0;
            ctx.scause = 0;
            ctx.sscratch = 0;
            // 运行时 sp：__restore 会 +304，故设为 kstack_top - 304
            // size_of<TrapContext> = 296，但 __restore 用 304，统一对齐
            ctx.x[2] = kstack_top - 304;
        }

        let task = Self {
            id,
            task_ctx: TaskContext {
                ra: __restore as usize,
                sp: ctx_ptr as usize,
                s: [0; 12],
            },
            kstack_top,
            state: TaskState::Ready,
            name,
        };
        Box::new(task)
    }
}

/// 切换：a0 = 当前 TaskContext 指针, a1 = 下一个 TaskContext 指针
#[naked_function]
pub unsafe extern "C" fn switch_to(_cur: *mut TaskContext, _next: *const TaskContext) {
    unsafe {
        core::arch::asm!(
            "j __switch",
            options(noreturn),
        );
    }
}
