mod frame;
mod heap;
mod page_table;
mod vm;

pub use frame::{alloc_frame, dealloc_frame, FrameTracker, PhysFrame};
pub use page_table::{PageTable, PageTableEntry, PTEFlags};
pub use vm::{MapArea, MapPerm, MapType, MemorySet, KERNEL_SPACE, KERNEL_SATP};

use crate::config::*;

/// 内核BSS段起始（由链接脚本提供）
extern "C" {
    fn sbss();
    fn ebss();
    fn ekernel();
}

pub fn init() {
    heap::init_heap();
    frame::init_frame_allocator();
    vm::init_kernel_space();
}

pub fn kernel_end_phys() -> usize {
    let va = ekernel as usize;
    crate::utils::virt_to_phys(va)
}
