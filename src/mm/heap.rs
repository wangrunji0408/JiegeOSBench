//! Kernel heap: a static region handed to linked_list_allocator.
use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

const HEAP_SIZE: usize = 64 * 1024 * 1024;

static mut HEAP_SPACE: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

pub fn init() {
    unsafe {
        HEAP.lock()
            .init(core::ptr::addr_of_mut!(HEAP_SPACE) as *mut u8, HEAP_SIZE);
    }
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("kernel heap OOM: {:?}", layout);
}
