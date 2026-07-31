//! Physical frame allocator: 4 KiB frames, free list stored inside free frames.

use core::sync::atomic::{AtomicBool, Ordering};

static LOCKED: AtomicBool = AtomicBool::new(false);

pub const FRAME_SIZE: usize = 4096;

pub static mut FRAME_START: usize = 0; // first free frame address
pub static mut FRAME_END: usize = 0;
static mut FREE_HEAD: usize = 0; // 0 = none

pub fn init(start: usize, end: usize) {
    let start = align_up(start, FRAME_SIZE);
    let end = align_down(end, FRAME_SIZE);
    unsafe {
        FRAME_START = start;
        FRAME_END = end;
        FREE_HEAD = 0;
    }
    // Build free list
    let mut addr = start;
    while addr < end {
        let next = if addr + FRAME_SIZE < end { addr + FRAME_SIZE } else { 0 };
        unsafe {
            (*(addr as *mut usize)) = next;
        }
        addr = if next == 0 { end } else { next };
    }
    unsafe {
        FREE_HEAD = start;
    }
}

fn lock() {
    while LOCKED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {}
}

fn unlock() {
    LOCKED.store(false, Ordering::SeqCst);
}

pub fn alloc_frame() -> Option<usize> {
    lock();
    let r = unsafe {
        if FREE_HEAD == 0 {
            None
        } else {
            let f = FREE_HEAD;
            FREE_HEAD = *(f as *const usize);
            Some(f)
        }
    };
    unlock();
    r
}

pub fn alloc_frames(n: usize) -> Option<usize> {
    // try to allocate n contiguous frames (best effort: from head)
    if n == 1 {
        return alloc_frame();
    }
    lock();
    let r = unsafe {
        let mut base = 0usize;
        // scan free list for n contiguous frames
        let mut prev = 0usize;
        let mut cur = FREE_HEAD;
        while cur != 0 {
            let next = *(cur as *const usize);
            // check contiguity
            if base != 0 && cur == base + FRAME_SIZE {
                // extend run
            } else {
                base = cur;
            }
            let mut run = 1;
            let mut p = cur;
            while run < n && p != 0 && *(p as *const usize) == p + FRAME_SIZE {
                p = *(p as *const usize);
                run += 1;
            }
            if run >= n {
                // remove first n frames starting at cur
                let mut p = cur;
                for _ in 0..n {
                    let nx = *(p as *const usize);
                    p = nx;
                }
                let tail = p;
                if prev == 0 {
                    FREE_HEAD = tail;
                } else {
                    *(prev as *mut usize) = tail;
                }
                return Some(cur);
            }
            prev = cur;
            cur = next;
        }
        None
    };
    unlock();
    r
}

pub fn free_frame(f: usize) {
    if f == 0 || f % FRAME_SIZE != 0 {
        return;
    }
    lock();
    unsafe {
        *(f as *mut usize) = FREE_HEAD;
        FREE_HEAD = f;
    }
    unlock();
}

pub fn free_frames(f: usize, n: usize) {
    for i in 0..n {
        free_frame(f + i * FRAME_SIZE);
    }
}

pub fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

pub fn align_down(x: usize, a: usize) -> usize {
    x & !(a - 1)
}
