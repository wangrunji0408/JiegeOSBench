// User address-space layout (identity-mapped, no MMU) and mmap bookkeeping.
use alloc::vec::Vec;

pub const PAGE: usize = 4096;

pub const MAIN_BASE: usize = 0x8800_0000;
pub const MAIN_LIMIT: usize = 0x8D00_0000;
pub const STACK_TOP: usize = 0x8F00_0000;
pub const STACK_SIZE: usize = 0x0080_0000; // 8 MiB
pub const MMAP_BASE: usize = 0x9000_0000;
pub const MMAP_END: usize = 0xA800_0000;
pub const INTERP_BASE: usize = 0xB000_0000;
pub const USER_TOP: usize = 0x1_0000_0000;

pub const MAX_HOLES: usize = 512;

static mut NEXT: usize = MMAP_BASE;
static mut HOLES: Vec<(usize, usize)> = Vec::new();

pub fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

pub fn init() {
    unsafe {
        NEXT = MMAP_BASE;
        HOLES.clear();
    }
}

pub fn bump_next() -> usize {
    unsafe { NEXT }
}

/// Allocate `len` bytes (page rounded) in the mmap region.
pub fn mmap_alloc(len: usize) -> Option<usize> {
    let len = align_up(len.max(1), PAGE);
    unsafe {
        // first fit in holes
        let mut best: Option<usize> = None;
        for (i, &(start, hlen)) in HOLES.iter().enumerate() {
            if hlen >= len {
                best = Some(i);
                break;
            }
        }
        if let Some(i) = best {
            let (start, hlen) = HOLES[i];
            if hlen - len < PAGE {
                HOLES.remove(i);
            } else {
                HOLES[i] = (start + len, hlen - len);
            }
            return Some(start);
        }
        if NEXT + len <= MMAP_END {
            let a = NEXT;
            NEXT += len;
            return Some(a);
        }
    }
    None
}

pub fn mmap_free(addr: usize, len: usize) {
    let len = align_up(len.max(1), PAGE);
    unsafe {
        if addr < MMAP_BASE || addr >= MMAP_END {
            return;
        }
        HOLES.push((addr, len));
        // merge adjacent holes
        HOLES.sort_by_key(|h| h.0);
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for &(s, l) in HOLES.iter() {
            if let Some(last) = merged.last_mut() {
                if last.0 + last.1 == s {
                    last.1 += l;
                    continue;
                }
            }
            merged.push((s, l));
        }
        HOLES = merged;
    }
}

pub fn in_user_range(ptr: usize, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let end = match ptr.checked_add(len) {
        Some(e) => e,
        None => return false,
    };
    ptr >= MAIN_BASE && end <= USER_TOP
}
