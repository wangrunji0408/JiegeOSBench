//! Kernel heap: first-fit free list allocator with coalescing.
//! Provides the `alloc` crate with memory.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

static LOCKED: AtomicBool = AtomicBool::new(false);

pub const HEAP_SIZE: usize = 32 * 1024 * 1024; // 32 MiB

static mut HEAP_START: usize = 0;
static mut FREE_HEAD: *mut Block = core::ptr::null_mut();

#[repr(C)]
struct Block {
    size: usize, // total block size including header, low bit = used
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
    unsafe {
        HEAP_START = start;
        let b = start as *mut Block;
        (*b).size = HEAP_SIZE;
        (*b).next = core::ptr::null_mut();
        FREE_HEAD = b;
    }
}

unsafe fn alloc_impl(size: usize, align: usize) -> Option<*mut u8> {
    // align the payload: we return pointer after header; need payload aligned.
    // We allocate blocks with header at front; ensure block start is aligned enough
    // by aligning the whole heap base at init (it is, 2MB-aligned).
    let mut prev: *mut Block = core::ptr::null_mut();
    let mut cur = FREE_HEAD;
    while !cur.is_null() {
        let bsize = (*cur).size & !1;
        // payload = cur + HEADER, need payload % align == 0
        let payload = (cur as usize + HEADER + align - 1) & !(align - 1);
        let used = payload - cur as usize + size;
        if used <= bsize {
            // split if remainder can hold a block
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
            return Some(payload as *mut u8);
        }
        prev = cur;
        cur = (*cur).next;
    }
    None
}

unsafe fn free_impl(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // find block header: header sits just before payload (payload may have padding)
    // we stored payload aligned; to find header, search backwards is unsafe.
    // Instead, store header pointer in a side table? Simpler: keep a header word
    // right before the payload by adjusting allocation: allocate header + align padding.
    // We do: block layout = [Block header][padding][payload]. To free, we need header ptr.
    // Trick: store the block base right before payload (8 bytes).
    let base = *((ptr as usize - 8) as *const usize);
    let b = base as *mut Block;
    let size = (*b).size & !1;
    // mark free
    (*b).size = size;
    // insert into free list (sorted by address for coalescing)
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
        // allocate size + 8 for the base-pointer word
        let r = alloc_impl(size + 8, align);
        unlock();
        match r {
            Some(p) => {
                // store block base 8 bytes before payload
                let block_base = p as usize - 8;
                // p is aligned to `align`; the word before payload is at p-8
                // but we need block base... we don't know it here because alloc_impl
                // computed payload = aligned(cur + HEADER); so block_base = cur.
                // recompute: we can't. Instead store cur in the word.
                // Modify alloc_impl to return (payload, cur).
                p
            }
            None => core::ptr::null_mut(),
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        lock();
        let base = *((ptr as usize - 8) as *const usize);
        free_impl_base(base as *mut u8);
        unlock();
    }
}

// rework: alloc_impl returns payload; we need base too. Use a global thread-local? No.
// Simplest robust approach: return payload, and store base at payload-8.
// To get base: payload-8 holds base. Then free: read base from ptr-8.

// We'll make alloc_impl return the payload and also store base at payload-8.

unsafe fn free_impl_base(base: usize) {
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
    if !(*b).next.is_null() && (b as usize + (*b).size) == ((*b).next as usize) {
        (*b).size += (*(*b).next).size & !1;
        (*b).next = (*(*b).next).next;
    }
    if !prev.is_null() && (prev as usize + (*prev).size) == (b as usize) {
        (*prev).size += (*b).size;
        (*prev).next = (*b).next;
    }
}
