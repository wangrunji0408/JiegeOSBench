pub mod frame;
pub mod heap;
pub mod paging;

use paging::PageTable;

pub static mut KERNEL_PT: Option<PageTable> = None;

pub fn init(kernel_end: usize, ram_end: usize) {
    // 1. Reserve the kernel image region; everything from kernel_end..ram_end is free frames.
    let free_start = paging::PAGE_SIZE * (frame::align_up(kernel_end, paging::PAGE_SIZE) / paging::PAGE_SIZE);
    frame::init(free_start, ram_end);

    // 2. Kernel heap: 32 MiB from free frames.
    let heap_start = frame::alloc_frames(heap::HEAP_SIZE / frame::FRAME_SIZE)
        .expect("cannot reserve kernel heap");
    heap::init(heap_start);

    // 3. Build the kernel page table: identity map RAM + MMIO.
    let mut pt = PageTable::new().expect("oom root pt");
    let mut addr = 0x8000_0000;
    while addr < ram_end {
        pt.map_2mb(addr, addr, paging::PTE_R | paging::PTE_W | paging::PTE_X);
        addr += paging::HUGE_PAGE_SIZE;
    }
    // MMIO: UART 0x10000000, virtio 0x10001000 (map a 64 KiB window), PLIC 0x0C000000
    let mut a = 0x1000_0000;
    while a < 0x1001_0000 {
        pt.map(a, a, paging::PTE_R | paging::PTE_W);
        a += paging::PAGE_SIZE;
    }
    let mut a = 0x0C00_0000;
    while a < 0x0C20_1000 {
        pt.map(a, a, paging::PTE_R | paging::PTE_W);
        a += paging::PAGE_SIZE;
    }
    unsafe {
        KERNEL_PT = Some(pt);
    }
}

pub fn kernel_pt() -> &'static mut PageTable {
    unsafe { KERNEL_PT.as_mut().unwrap() }
}
