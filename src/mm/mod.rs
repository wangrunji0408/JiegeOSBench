//! 内存管理子模块：物理帧分配器、Sv39 页表、内核堆。

pub mod frame;
pub mod page_table;
pub mod heap;
pub mod address;

pub use frame::FRAME_ALLOCATOR;
pub use page_table::{PageTable, PAGE_SIZE};

/// QEMU virt 内存布局
pub const PHYS_RAM_BASE: usize = 0x8000_0000;
pub const MEMORY_TOP: usize = 0x8800_0000; // 128MB 物理内存上限（与 -m 128M 对应）
pub const HEAP_START: usize = 0x8080_0000; // 堆物理起始（约 6MB 偏移，远离内核镜像）
pub const HEAP_SIZE: usize = 16 * 1024 * 1024; // 16MB 内核堆

/// 初始化整个内存子系统
pub fn init() {
    let kernel_end = unsafe { (__kernel_end as *const () as usize) };
    frame::init(kernel_end);
    heap::init();
    page_table::init_kernel();
}

extern "C" {
    fn __kernel_end();
}

pub fn kernel_end_pa() -> usize {
    unsafe { __kernel_end as *const () as usize }
}
