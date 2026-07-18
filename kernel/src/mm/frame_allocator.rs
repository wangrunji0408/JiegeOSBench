//! Physical frame allocator: a simple stack-based free-list allocator over
//! [ekernel, MEMORY_END).

use super::address::{PhysAddr, PhysPageNum};
use crate::config::MEMORY_END;
use alloc::vec::Vec;
use spin::Mutex;

pub struct FrameTracker {
    pub ppn: PhysPageNum,
}

impl FrameTracker {
    fn new(ppn: PhysPageNum) -> Self {
        let bytes = ppn.as_bytes();
        bytes.fill(0);
        Self { ppn }
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        FRAME_ALLOCATOR.lock().dealloc(self.ppn);
    }
}

struct StackFrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}

impl StackFrameAllocator {
    const fn empty() -> Self {
        Self {
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }

    fn init(&mut self, start: PhysPageNum, end: PhysPageNum) {
        self.current = start.0;
        self.end = end.0;
    }

    fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.recycled.pop() {
            Some(PhysPageNum(ppn))
        } else if self.current < self.end {
            self.current += 1;
            Some(PhysPageNum(self.current - 1))
        } else {
            None
        }
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        debug_assert!(
            ppn < self.current && !self.recycled.iter().any(|&v| v == ppn),
            "frame ppn={:#x} double free or invalid",
            ppn
        );
        self.recycled.push(ppn);
    }
}

static FRAME_ALLOCATOR: Mutex<StackFrameAllocator> = Mutex::new(StackFrameAllocator::empty());

pub fn init_frame_allocator() {
    unsafe extern "C" {
        fn ekernel();
    }
    let start = PhysAddr::from(ekernel as usize as usize).ceil();
    let end = PhysAddr::from(MEMORY_END).floor();
    FRAME_ALLOCATOR.lock().init(start, end);
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR.lock().alloc().map(FrameTracker::new)
}

pub fn frame_alloc_contig(count: usize) -> Option<Vec<FrameTracker>> {
    // Only used for virtio DMA buffers which need physically-contiguous
    // memory; fall back to a dedicated bump range taken directly from the
    // allocator's current watermark since our simple stack allocator cannot
    // otherwise guarantee contiguity once frees have interleaved the free list.
    let mut guard = FRAME_ALLOCATOR.lock();
    if guard.current + count > guard.end {
        return None;
    }
    let start = guard.current;
    guard.current += count;
    drop(guard);
    Some(
        (start..start + count)
            .map(|ppn| FrameTracker::new(PhysPageNum(ppn)))
            .collect(),
    )
}
