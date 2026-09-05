//! Kernel heap: all free RAM is handed to a buddy allocator. Page frames for
//! user memory are allocated from the same heap with page alignment.
use buddy_system_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap<32> = LockedHeap::empty();

pub unsafe fn add_region(start: usize, end: usize) {
    if end > start {
        HEAP.lock().add_to_heap(start, end);
    }
}

pub fn stats() -> (usize, usize) {
    let h = HEAP.lock();
    (h.stats_alloc_actual(), h.stats_total_bytes())
}
