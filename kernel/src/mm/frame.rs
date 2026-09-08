//! Physical frame allocator: a simple bitmap over physical RAM.
//!
//! The kernel identity-maps RAM, so a frame's physical address is also a valid
//! kernel virtual address.

use crate::mm::address::PAGE_SIZE;
use crate::sync::SpinLock;

const MAX_FRAMES: usize = 1 << 20; // 4 GiB worth of 4 KiB frames
const WORDS: usize = MAX_FRAMES / 64;

struct FrameAlloc {
    bitmap: [u64; WORDS],
    base: usize,
    frames: usize,
    free: usize,
}

static FRAMES: SpinLock<FrameAlloc> = SpinLock::new(FrameAlloc {
    bitmap: [u64::MAX; WORDS],
    base: 0,
    frames: 0,
    free: 0,
});

#[inline]
fn test_bit(bm: &[u64], i: usize) -> bool {
    bm[i / 64] & (1u64 << (i % 64)) != 0
}

#[inline]
fn set_bit(bm: &mut [u64], i: usize) {
    bm[i / 64] |= 1u64 << (i % 64);
}

#[inline]
fn clear_bit(bm: &mut [u64], i: usize) {
    bm[i / 64] &= !(1u64 << (i % 64));
}

/// Initialise the allocator: everything in `[ram_start, ram_end)` is free except
/// the given reserved ranges.
pub fn init(ram_start: usize, ram_end: usize, reserved: &[(usize, usize)]) {
    let mut f = FRAMES.lock();
    f.base = ram_start;
    f.frames = (ram_end - ram_start) / PAGE_SIZE;
    if f.frames > MAX_FRAMES {
        f.frames = MAX_FRAMES;
    }
    for w in f.bitmap.iter_mut() {
        *w = u64::MAX;
    }
    for i in 0..f.frames {
        clear_bit(&mut f.bitmap, i);
    }
    f.free = f.frames;
    for &(s, e) in reserved {
        let s = s.max(ram_start);
        let e = e.min(ram_end);
        let mut i = (s - ram_start) / PAGE_SIZE;
        let end = (e - ram_start + PAGE_SIZE - 1) / PAGE_SIZE;
        while i < end && i < f.frames {
            if !test_bit(&f.bitmap, i) {
                set_bit(&mut f.bitmap, i);
                f.free -= 1;
            }
            i += 1;
        }
    }
    crate::println!(
        "[mm] frames: {} total, {} free ({} MiB)",
        f.frames,
        f.free,
        f.free * PAGE_SIZE / 1024 / 1024
    );
}

/// Allocate one zeroed 4 KiB frame, returning its physical address.
pub fn alloc_frame() -> Option<usize> {
    alloc_frames(1)
}

/// Allocate `n` contiguous zeroed frames.
pub fn alloc_frames(n: usize) -> Option<usize> {
    let mut f = FRAMES.lock();
    let mut run = 0usize;
    let mut i = 0usize;
    while i < f.frames {
        if !test_bit(&f.bitmap, i) {
            run += 1;
            if run == n {
                let start = i + 1 - n;
                for j in start..=i {
                    set_bit(&mut f.bitmap, j);
                }
                f.free -= n;
                let pa = f.base + start * PAGE_SIZE;
                drop(f);
                unsafe {
                    core::ptr::write_bytes(pa as *mut u8, 0, n * PAGE_SIZE);
                }
                return Some(pa);
            }
        } else {
            run = 0;
        }
        i += 1;
    }
    None
}

pub fn free_frame(pa: usize) {
    let mut f = FRAMES.lock();
    if pa < f.base {
        return;
    }
    let i = (pa - f.base) / PAGE_SIZE;
    if i < f.frames && test_bit(&f.bitmap, i) {
        clear_bit(&mut f.bitmap, i);
        f.free += 1;
    }
}

pub fn free_frames(pa: usize, n: usize) {
    for k in 0..n {
        free_frame(pa + k * PAGE_SIZE);
    }
}

pub fn free_count() -> usize {
    FRAMES.lock().free
}

pub fn total_count() -> usize {
    FRAMES.lock().frames
}
