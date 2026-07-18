//! 内核堆（位于 .bss 的静态区域 + buddy 分配器）

use crate::config::KERNEL_HEAP_SIZE;
use buddy_system_allocator::LockedHeap;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::<32>::empty();

static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];

pub fn init_heap() {
    unsafe {
        let start = core::ptr::addr_of_mut!(HEAP_SPACE) as usize;
        HEAP_ALLOCATOR
            .lock()
            .init(start, KERNEL_HEAP_SIZE);
    }
    // 简单测试
    {
        use alloc::boxed::Box;
        let x = Box::new(5);
        assert_eq!(*x, 5);
    }
    println!("kernel heap initialized: {} MiB", KERNEL_HEAP_SIZE / 1024 / 1024);
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("kernel heap allocation error: {:?}", layout)
}
