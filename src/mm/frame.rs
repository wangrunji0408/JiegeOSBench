//! Physical frame allocator.
//!
//! A simple free-list allocator over the physical frames not consumed by the
//! kernel image. Frames are reference counted so that fork/COW and shared file
//! mappings can share them.

use super::addr::*;
use alloc::vec::Vec;
use spin::Mutex;

struct FrameAllocator {
    /// Frames never yet handed out: `[next, end)`.
    next: usize,
    end: usize,
    /// Frames returned to us, available for immediate reuse.
    recycled: Vec<usize>,
    /// Reference count per frame, indexed by `(pa - base) >> PAGE_SHIFT`.
    refcount: Vec<u16>,
    base: usize,
    allocated: usize,
}

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            next: 0,
            end: 0,
            recycled: Vec::new(),
            refcount: Vec::new(),
            base: 0,
            allocated: 0,
        }
    }

    fn init(&mut self, start: usize, end: usize) {
        self.base = start;
        self.next = start;
        self.end = end;
        let frames = (end - start) >> PAGE_SHIFT;
        self.refcount = alloc::vec![0u16; frames];
    }

    #[inline]
    fn index(&self, pa: usize) -> usize {
        (pa - self.base) >> PAGE_SHIFT
    }

    fn alloc(&mut self) -> Option<usize> {
        let pa = if let Some(pa) = self.recycled.pop() {
            pa
        } else if self.next < self.end {
            let pa = self.next;
            self.next += PAGE_SIZE;
            pa
        } else {
            return None;
        };
        let idx = self.index(pa);
        debug_assert_eq!(self.refcount[idx], 0, "allocating a live frame");
        self.refcount[idx] = 1;
        self.allocated += 1;
        Some(pa)
    }

    fn incref(&mut self, pa: usize) {
        let idx = self.index(pa);
        self.refcount[idx] = self.refcount[idx].saturating_add(1);
    }

    /// Drop a reference. Returns true if the frame became free.
    fn decref(&mut self, pa: usize) -> bool {
        let idx = self.index(pa);
        let rc = self.refcount[idx];
        if rc == 0 {
            // Double free; ignore rather than corrupt state.
            return false;
        }
        self.refcount[idx] = rc - 1;
        if rc == 1 {
            self.recycled.push(pa);
            self.allocated -= 1;
            true
        } else {
            false
        }
    }

    fn refcount(&self, pa: usize) -> u16 {
        self.refcount[self.index(pa)]
    }
}

static ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

pub fn init(start: usize, end: usize) {
    ALLOCATOR.lock().init(page_up(start), page_down(end));
}

/// Allocate one zeroed physical frame. Returns the physical address.
pub fn alloc_frame() -> Option<usize> {
    let pa = ALLOCATOR.lock().alloc()?;
    unsafe {
        core::ptr::write_bytes(phys_to_virt(pa) as *mut u8, 0, PAGE_SIZE);
    }
    Some(pa)
}

/// Allocate a frame without zeroing it (caller will overwrite all of it).
pub fn alloc_frame_dirty() -> Option<usize> {
    ALLOCATOR.lock().alloc()
}

pub fn incref(pa: usize) {
    ALLOCATOR.lock().incref(pa);
}

pub fn decref(pa: usize) {
    ALLOCATOR.lock().decref(pa);
}

pub fn refcount(pa: usize) -> u16 {
    ALLOCATOR.lock().refcount(pa)
}

/// (allocated_frames, total_frames)
pub fn stats() -> (usize, usize) {
    let a = ALLOCATOR.lock();
    let total = (a.end - a.base) >> PAGE_SHIFT;
    (a.allocated, total)
}
