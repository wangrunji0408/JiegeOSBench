//! Kernel heap: backs Rust's `alloc` (Vec/Box/BTreeMap/...) with a static
//! arena managed by a buddy allocator.

use crate::config::KERNEL_HEAP_SIZE;
use buddy_system_allocator::LockedHeap;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::empty();

static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0u8; KERNEL_HEAP_SIZE];

pub fn init() {
    unsafe {
        let base = core::ptr::addr_of_mut!(HEAP_SPACE) as usize;
        HEAP_ALLOCATOR.lock().init(base, KERNEL_HEAP_SIZE);
    }
}
