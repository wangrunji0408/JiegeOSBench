//! Kernel heap backed by the buddy system allocator.

use crate::sync::SpinLock;
use buddy_system_allocator::Heap;

pub const HEAP_SIZE: usize = 32 * 1024 * 1024; // 32 MiB

/// Static zero-initialized heap region (lives in .bss).
#[repr(align(4096))]
struct HeapMem([u8; HEAP_SIZE]);
static mut HEAP_MEM: HeapMem = HeapMem([0; HEAP_SIZE]);

static HEAP: SpinLock<Heap<32>> = SpinLock::new(Heap::<32>::new());

pub fn init() {
    unsafe {
        HEAP.lock().init(HEAP_MEM.0.as_ptr() as usize, HEAP_SIZE);
    }
}

pub struct GlobalAllocator;

unsafe impl core::alloc::GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let mut heap = HEAP.lock();
        match heap.alloc(layout) {
            Ok(ptr) => ptr.as_ptr(),
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        let mut heap = HEAP.lock();
        heap.dealloc(core::ptr::NonNull::new_unchecked(ptr), layout);
    }
}
