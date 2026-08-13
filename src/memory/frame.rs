//! Physical frame allocator (4 KiB frames) implemented as a free list.

use crate::sync::SpinLock;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysAddr(pub usize);

pub struct FrameAllocator {
    /// Head of the free list. Each free frame's first word points to the next.
    free_head: Option<PhysAddr>,
    total: usize,
    used: usize,
}

static FRAME_ALLOCATOR: SpinLock<FrameAllocator> = SpinLock::new(FrameAllocator {
    free_head: None,
    total: 0,
    used: 0,
});

pub fn init() {
    extern "C" {
        static mut _end: u8;
    }
    let start = align_up(&raw const _end as usize, PAGE_SIZE);
    let end = crate::memory::MEMORY_END;

    let mut fa = FRAME_ALLOCATOR.lock();
    fa.free_head = None;
    fa.total = 0;
    fa.used = 0;

    let mut cur = start;
    while cur + PAGE_SIZE <= end {
        unsafe {
            *(cur as *mut usize) = fa.free_head.map(|p| p.0).unwrap_or(0);
        }
        fa.free_head = Some(PhysAddr(cur));
        fa.total += 1;
        cur += PAGE_SIZE;
    }
    crate::println!(
        "[mem] frame allocator: {} frames ({} MiB) from {:#x} to {:#x}",
        fa.total,
        fa.total * PAGE_SIZE / (1024 * 1024),
        start,
        end
    );
}

pub fn alloc() -> Option<PhysAddr> {
    let mut fa = FRAME_ALLOCATOR.lock();
    let f = fa.free_head?;
    let next = unsafe { *(f.0 as *const usize) };
    fa.free_head = if next == 0 { None } else { Some(PhysAddr(next)) };
    fa.used += 1;
    // zero the frame
    unsafe { core::slice::from_raw_parts_mut(f.0 as *mut u8, PAGE_SIZE).fill(0) };
    Some(f)
}

pub fn dealloc(f: PhysAddr) {
    let mut fa = FRAME_ALLOCATOR.lock();
    unsafe {
        *(f.0 as *mut usize) = fa.free_head.map(|p| p.0).unwrap_or(0);
    }
    fa.free_head = Some(f);
    fa.used -= 1;
}

pub fn free_count() -> usize {
    let fa = FRAME_ALLOCATOR.lock();
    fa.total - fa.used
}

pub fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}
