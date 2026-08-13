//! Memory management: frame allocator, Sv39 page tables, kernel heap.

pub mod frame;
pub mod heap;
pub mod page_table;

pub use page_table::PageTable;

/// Physical RAM bounds for QEMU `virt` with `-m 256M`.
pub const MEMORY_START: usize = 0x8000_0000;
pub const MEMORY_END: usize = 0x9000_0000;

/// MMIO regions to identity-map for kernel access.
/// (start, end): PLIC, UART0 + virtio MMIO.
pub const MMIO_REGIONS: &[(usize, usize)] = &[
    (0x0c00_0000, 0x0c40_0000), // PLIC
    (0x1000_0000, 0x1000_8000), // UART0 + virtio MMIO
];

/// Initialize memory: frame allocator + kernel heap, then set up the kernel
/// page table and enable Sv39 paging.
pub fn init() {
    frame::init();
    heap::init();

    let mut pt = page_table::kernel_page_table();
    // Activate paging.
    unsafe {
        let satp = pt.satp();
        core::arch::asm!("csrw satp, {}", in(reg) satp);
        core::arch::asm!("sfence.vma");
    }
    crate::println!(
        "[mem] paging enabled (satp={:#x}), heap={} bytes, frames {} free",
        pt.satp(),
        heap::HEAP_SIZE,
        frame::free_count(),
    );
    // Keep the kernel page table alive.
    core::mem::forget(pt);
}
