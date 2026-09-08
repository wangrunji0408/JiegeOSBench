//! Kernel heap on top of the frame allocator.

use crate::mm::frame;
use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

const HEAP_SIZE: usize = 64 * 1024 * 1024;

pub fn init() {
    let base = frame::alloc_frames(HEAP_SIZE / crate::mm::address::PAGE_SIZE)
        .expect("failed to reserve kernel heap");
    unsafe {
        HEAP.lock().init(base as *mut u8, HEAP_SIZE);
    }
    crate::println!("[mm] kernel heap at {:#x} ({} MiB)", base, HEAP_SIZE / 1024 / 1024);
}
