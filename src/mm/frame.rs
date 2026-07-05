//! Physical frame allocator over RAM after the kernel image.
use super::PAGE_SIZE;
use alloc::vec::Vec;
use spin::Mutex;

pub const RAM_END: usize = 0xC000_0000; // 1 GiB RAM at 0x8000_0000

struct FrameAlloc {
    next: usize,
    end: usize,
    freed: Vec<usize>,
}

static ALLOC: Mutex<FrameAlloc> = Mutex::new(FrameAlloc {
    next: 0,
    end: 0,
    freed: Vec::new(),
});

pub fn init() {
    extern "C" {
        static __kernel_end: u8;
    }
    let kernel_end = unsafe { core::ptr::addr_of!(__kernel_end) as usize };
    let mut a = ALLOC.lock();
    a.next = super::page_up(kernel_end);
    a.end = RAM_END;
    println!(
        "[mm] frame allocator: {:#x}..{:#x} ({} MiB)",
        a.next,
        a.end,
        (a.end - a.next) / 1024 / 1024
    );
}

/// Allocate one zeroed 4K frame; returns its physical address (== kernel VA).
pub fn alloc() -> usize {
    let pa = {
        let mut a = ALLOC.lock();
        if let Some(pa) = a.freed.pop() {
            pa
        } else {
            let pa = a.next;
            if pa + PAGE_SIZE > a.end {
                panic!("out of physical frames");
            }
            a.next += PAGE_SIZE;
            pa
        }
    };
    unsafe { core::ptr::write_bytes(pa as *mut u8, 0, PAGE_SIZE) };
    pa
}

pub fn free(pa: usize) {
    ALLOC.lock().freed.push(pa);
}
