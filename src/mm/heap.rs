//! Kernel heap.

use buddy_system_allocator::LockedHeap;

/// 64 MiB of kernel heap, statically reserved in `.bss`. The network stack's
/// packet buffers and the in-memory filesystem holding nginx and its libraries
/// (~10 MiB) both live here.
const KERNEL_HEAP_SIZE: usize = 64 * 1024 * 1024;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::empty();

static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];

pub fn init() {
    unsafe {
        let start = HEAP_SPACE.as_mut_ptr() as usize;
        HEAP_ALLOCATOR.lock().init(start, KERNEL_HEAP_SIZE);
    }
}

/// Bytes currently allocated from the kernel heap.
pub fn used() -> usize {
    HEAP_ALLOCATOR.lock().stats_alloc_actual()
}

pub fn total() -> usize {
    KERNEL_HEAP_SIZE
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!(
        "kernel heap exhausted allocating {} bytes (align {}); {}/{} used",
        layout.size(),
        layout.align(),
        used(),
        total()
    );
}
