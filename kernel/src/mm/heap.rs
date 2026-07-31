//! Kernel heap: first-fit free list allocator with coalescing.
//! Block layout: [Block header (size|used, next)] [padding] [base word (8B)] [payload]
//! The base word lets free() recover the block header from the aligned payload pointer.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

static LOCKED: AtomicBool = AtomicBool::new(false);

pub const HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

static mut FREE_HEAD: *mut Block = core::ptr::null_mut();

#[repr(C)]
struct Block {
    size: usize, // total block size including header; low bit = used
    next: *mut Block,
}

const HEADER: usize = core::mem::size_of::<Block>();

fn lock() {
    while LOCKED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {}
}

fn unlock() {
    LOCKED.store(false, Ordering::SeqCst);
}

pub fn init(start: usize) {
    lock();
    unsafe {
        let b = start as *mut Block;
        (*b).size = HEAP_SIZE;
        (*b).next = core::ptr::null_mut();
        FREE_HEAD = b;
    }
    unlock();
}

pub fn dbg_head() -> usize {
    unsafe { FREE_HEAD as usize }
}

unsafe fn alloc_impl(size: usize, align: usize) -> Option<*mut u8> {
    let mut prev: *mut Block = core::ptr::null_mut();
    let mut cur = FREE_HEAD;
    let mut hops = 0;
    while !cur.is_null() {
        if (cur as usize) % 8 != 0 || hops > 100000 {
            crate::kprintln!("[heap] FREE LIST CORRUPT: cur={:#x} prev={:#x} hops={} size_req={}", cur as usize, prev as usize, hops, size);
            break;
        }
        hops += 1;
        let bsize = (*cur).size & !1;
        let payload = (cur as usize + HEADER + align - 1) & !(align - 1);
        // bytes consumed: header + padding + 8-byte base word + payload;
        // round up to 8 so the remainder block stays 8-byte aligned
        let used = (payload - cur as usize + 8 + size + 7) & !7;
        if used <= bsize {
            if bsize - used >= HEADER + 8 {
                let rem = (cur as usize + used) as *mut Block;
                (*rem).size = bsize - used;
                (*rem).next = (*cur).next;
                (*cur).size = used | 1;
                if prev.is_null() {
                    FREE_HEAD = rem;
                } else {
                    (*prev).next = rem;
                }
            } else {
                let whole = bsize | 1;
                if prev.is_null() {
                    FREE_HEAD = (*cur).next;
                } else {
                    (*prev).next = (*cur).next;
                }
                (*cur).size = whole;
            }
            // store block base before payload
            *((payload - 8) as *mut usize) = cur as usize;
            return Some(payload as *mut u8);
        }
        prev = cur;
        cur = (*cur).next;
    }
    None
}

unsafe fn free_impl(ptr: *mut u8) {
    let base = *((ptr as usize - 8) as *const usize);
    let b = base as *mut Block;
    let size = (*b).size & !1;
    (*b).size = size;
    let mut prev: *mut Block = core::ptr::null_mut();
    let mut cur = FREE_HEAD;
    while !cur.is_null() && (cur as usize) < (b as usize) {
        prev = cur;
        cur = (*cur).next;
    }
    if prev.is_null() {
        FREE_HEAD = b;
    } else {
        (*prev).next = b;
    }
    (*b).next = cur;
    // coalesce with next
    if !(*b).next.is_null() && (b as usize + (*b).size) == ((*b).next as usize) {
        (*b).size += (*(*b).next).size & !1;
        (*b).next = (*(*b).next).next;
    }
    // coalesce with prev
    if !prev.is_null() && (prev as usize + (*prev).size) == (b as usize) {
        (*prev).size += (*b).size;
        (*prev).next = (*b).next;
    }
}

pub struct HeapAllocator;

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        lock();
        let size = layout.size();
        let align = layout.align().max(8);
        let r = alloc_impl(size, align);
        unlock();
        r.unwrap_or(core::ptr::null_mut())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }
        lock();
        free_impl(ptr);
        unlock();
    }
}
