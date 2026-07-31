pub mod frame;
pub mod heap;
pub mod paging;
pub mod vma;

use paging::PageTable;

pub static mut KERNEL_PT: Option<PageTable> = None;
pub static mut RAM_END: usize = 0;

pub fn init(kernel_end: usize, ram_end: usize) {
    unsafe {
        RAM_END = ram_end;
    }
    // Everything from kernel_end..ram_end is free frames.
    let free_start = paging::PAGE_SIZE * (frame::align_up(kernel_end, paging::PAGE_SIZE) / paging::PAGE_SIZE);
    frame::init(free_start, ram_end);

    // 3. Build the kernel page table: identity map RAM + MMIO.
    let mut pt = PageTable::new().expect("oom root pt");
    map_kernel_into(&mut pt);
    unsafe {
        KERNEL_PT = Some(pt);
    }
}

/// Identity-map RAM (2 MiB huge pages) and MMIO windows into a page table.
pub fn map_kernel_into(pt: &mut PageTable) {
    let ram_end = unsafe { RAM_END };
    let mut addr = 0x8000_0000;
    while addr < ram_end {
        pt.map_2mb(addr, addr, paging::PTE_R | paging::PTE_W | paging::PTE_X);
        addr += paging::HUGE_PAGE_SIZE;
    }
    // MMIO: UART + virtio 0x10000000..0x10010000, PLIC 0x0C000000..0x0C201000
    let mut a = 0x1000_0000;
    while a < 0x1001_0000 {
        pt.map(a, a, paging::PTE_R | paging::PTE_W);
        a += paging::PAGE_SIZE;
    }
    let mut a = 0x0C00_0000;
    while a < 0x0C20_2000 {
        pt.map(a, a, paging::PTE_R | paging::PTE_W);
        a += paging::PAGE_SIZE;
    }
}

pub fn kernel_pt() -> &'static mut PageTable {
    unsafe { KERNEL_PT.as_mut().unwrap() }
}
