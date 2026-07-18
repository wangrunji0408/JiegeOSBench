pub mod address;
pub mod frame_allocator;
pub mod heap_allocator;
pub mod memory_set;
pub mod page_table;

use spin::Once;

pub use address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
pub use memory_set::{MapArea, MapPermission, MapType, MemorySet};
pub use page_table::{PTEFlags, PageTable};
pub use page_table::translated_byte_buffer;

static KERNEL_SPACE: Once<spin::Mutex<MemorySet>> = Once::new();

pub fn kernel_token() -> usize {
    KERNEL_SPACE.get().unwrap().lock().token()
}

pub fn init() {
    heap_allocator::init();
    frame_allocator::init_frame_allocator();
    KERNEL_SPACE.call_once(|| spin::Mutex::new(MemorySet::new_kernel()));
    KERNEL_SPACE.get().unwrap().lock().activate();
}
