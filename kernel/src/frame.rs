use crate::config::{PAGE_SIZE, PHYS_END};
use alloc::vec::Vec;
use spin::Mutex;

/// Physical frame allocator: a bump pointer for fresh frames plus a free list
/// of recycled frames. Frames are identified by physical address (page-aligned).
struct FrameAllocator {
    next: usize,
    end: usize,
    freed: Vec<usize>,
}

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            next: 0,
            end: 0,
            freed: Vec::new(),
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        if let Some(pa) = self.freed.pop() {
            Some(pa)
        } else if self.next < self.end {
            let pa = self.next;
            self.next += PAGE_SIZE;
            Some(pa)
        } else {
            None
        }
    }

    fn free(&mut self, pa: usize) {
        self.freed.push(pa);
    }

    fn alloc_contig(&mut self, pages: usize) -> Option<usize> {
        // Contiguous allocations come from the bump region only.
        let start = self.next;
        if start + pages * PAGE_SIZE <= self.end {
            self.next = start + pages * PAGE_SIZE;
            Some(start)
        } else {
            None
        }
    }
}

static ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

pub fn init(start: usize) {
    let mut a = ALLOCATOR.lock();
    a.next = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    a.end = PHYS_END;
}

/// Allocate a zeroed physical frame; returns its physical address.
pub fn alloc() -> usize {
    let pa = ALLOCATOR.lock().alloc().expect("out of physical frames");
    unsafe {
        core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE);
    }
    pa
}

pub fn free(pa: usize) {
    ALLOCATOR.lock().free(pa);
}

/// Allocate `pages` contiguous zeroed frames; returns the base physical address.
pub fn alloc_contig(pages: usize) -> usize {
    let pa = ALLOCATOR
        .lock()
        .alloc_contig(pages)
        .expect("out of contiguous frames");
    unsafe {
        core::ptr::write_bytes(pa as *mut u8, 0, pages * PAGE_SIZE);
    }
    pa
}

pub fn free_count() -> usize {
    let a = ALLOCATOR.lock();
    (a.end - a.next) / PAGE_SIZE + a.freed.len()
}
