//! 内核堆：物理内存上的一块区域 + 简单 first-fit 分配器

use alloc::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

const MIN_ALIGN: usize = 16;

struct Node {
    size: usize, // 本块总大小（含头）
    free: bool,
}

static mut HEAP_START: usize = 0;
static mut HEAP_SIZE: usize = 0;
static LOCK: AtomicBool = AtomicBool::new(false);

pub fn init(start: usize, size: usize) {
    unsafe {
        HEAP_START = start;
        HEAP_SIZE = size;
        let n = start as *mut Node;
        (*n).size = size;
        (*n).free = true;
    }
}

unsafe fn lock_heap() {
    while LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}
unsafe fn unlock_heap() {
    LOCK.store(false, Ordering::Release);
}

struct KernelAlloc;

unsafe impl GlobalAlloc for KernelAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let need = (layout.size() + layout.align().max(MIN_ALIGN) - 1) & !(layout.align().max(MIN_ALIGN) - 1);
        let total = need + core::mem::size_of::<Node>();
        let hdr = core::mem::size_of::<Node>();

        lock_heap();
        let start = HEAP_START;
        let end = start + HEAP_SIZE;
        let mut p = start;
        let mut result: *mut u8 = core::ptr::null_mut();
        while p + hdr <= end {
            let node = p as *mut Node;
            let node_size = (*node).size;
            if (*node).free && node_size >= total {
                if node_size >= total + hdr + MIN_ALIGN {
                    // 分裂
                    let rest = p + total;
                    (*node).size = total;
                    let rn = rest as *mut Node;
                    (*rn).size = node_size - total;
                    (*rn).free = true;
                }
                (*node).free = false;
                result = (p + hdr) as *mut u8;
                break;
            }
            p += node_size;
        }
        unlock_heap();
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        if ptr.is_null() {
            return;
        }
        lock_heap();
        let start = HEAP_START;
        let end = start + HEAP_SIZE;
        let hdr = core::mem::size_of::<Node>();
        let node = ptr.sub(hdr) as *mut Node;
        debug_assert!((*node).free == false);
        (*node).free = true;
        // 向后合并
        let mut p = node;
        loop {
            let sz = (*p).size;
            let next = (p as usize + sz) as *mut Node;
            if (next as usize) + hdr <= end && (*next).free {
                (*p).size = sz + (*next).size;
            } else {
                break;
            }
        }
        unlock_heap();
    }
}

#[global_allocator]
static ALLOCATOR: KernelAlloc = KernelAlloc;
