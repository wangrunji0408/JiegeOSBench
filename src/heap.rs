// A small first-fit free-list kernel heap allocator.
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

const HEAP_START: usize = 0x8400_0000;
const HEAP_SIZE: usize = 0x0200_0000; // 32 MiB

#[repr(C)]
struct Block {
    size: usize,
    next: *mut Block,
}

static mut FREE_HEAD: *mut Block = ptr::null_mut();

pub fn init() {
    unsafe {
        let b = HEAP_START as *mut Block;
        (*b).size = HEAP_SIZE;
        (*b).next = ptr::null_mut();
        FREE_HEAD = b;
    }
}

fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

unsafe fn insert_free(base: usize, size: usize) {
    if size < 32 {
        return;
    }
    let mut prev: *mut Block = ptr::null_mut();
    let mut cur = FREE_HEAD;
    while !cur.is_null() && (cur as usize) < base {
        prev = cur;
        cur = (*cur).next;
    }
    let mut node = base as *mut Block;
    (*node).size = size;
    (*node).next = cur;
    if prev.is_null() {
        FREE_HEAD = node;
    } else {
        (*prev).next = node;
    }
    // coalesce with next
    if !cur.is_null() && base + (*node).size == cur as usize {
        (*node).size += (*cur).size;
        (*node).next = (*cur).next;
    }
    // coalesce with previous
    if !prev.is_null() && (prev as usize) + (*prev).size == node as usize {
        (*prev).size += (*node).size;
        (*prev).next = (*node).next;
        node = prev;
    }
}

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(16);
        let size = layout.size().max(1);
        let mut prev: *mut Block = ptr::null_mut();
        let mut cur = FREE_HEAD;
        while !cur.is_null() {
            let base = cur as usize;
            let bsize = (*cur).size;
            let user = align_up(base + 16, align);
            let end = user + size;
            if end <= base + bsize {
                let next = (*cur).next;
                if prev.is_null() {
                    FREE_HEAD = next;
                } else {
                    (*prev).next = next;
                }
                let rem_start = align_up(end, 16);
                let alloc_size;
                if base + bsize >= rem_start + 64 {
                    alloc_size = rem_start - base;
                    insert_free(rem_start, base + bsize - rem_start);
                } else {
                    alloc_size = bsize;
                }
                let hp = (user - 16) as *mut usize;
                *hp = base;
                *hp.add(1) = alloc_size;
                return user as *mut u8;
            }
            prev = cur;
            cur = (*cur).next;
        }
        ptr::null_mut()
    }

    unsafe fn dealloc(&self, ptr_: *mut u8, _layout: Layout) {
        let hp = (ptr_ as usize - 16) as *const usize;
        let base = *hp;
        let size = *hp.add(1);
        insert_free(base, size);
    }
}

#[global_allocator]
pub static ALLOCATOR: KernelHeap = KernelHeap;
