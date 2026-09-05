use crate::config::KERNEL_HEAP_SIZE;
use core::alloc::Layout;
use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

/// Initialize the kernel heap over [start, start+size).
pub fn init(start: usize) -> usize {
    unsafe {
        HEAP.lock().init(start as *mut u8, KERNEL_HEAP_SIZE);
    }
    start + KERNEL_HEAP_SIZE
}

#[alloc_error_handler]
fn oom(layout: Layout) -> ! {
    panic!("kernel heap OOM: {:?}", layout);
}
