use crate::config::*;
use crate::page_table::*;
use crate::{frame, heap};
use core::arch::asm;
use spin::Once;

extern "C" {
    fn ekernel();
}

static KERNEL_PT: Once<PageTable> = Once::new();

/// Add the kernel's identity mappings (RAM + MMIO) to a page table.
/// These are shared by every address space so that traps never switch satp.
pub fn map_kernel(pt: &PageTable) {
    // Kernel RAM: one 1 GiB gigapage covering [0x8000_0000, 0xC000_0000).
    pt.map_at(PHYS_START, PHYS_START, PTE_R | PTE_W | PTE_X, 2);

    // MMIO (no U bit, no X): map as 4K pages.
    let mut map_mmio = |base: usize, size: usize| {
        let mut off = 0;
        while off < size {
            pt.map(base + off, base + off, PTE_R | PTE_W);
            off += PAGE_SIZE;
        }
    };
    map_mmio(UART_BASE, PAGE_SIZE);
    map_mmio(VIRTIO0_BASE, VIRTIO_COUNT * VIRTIO_STRIDE);
    map_mmio(CLINT_BASE, 0x1_0000);
    map_mmio(PLIC_BASE, 0x40_0000);
}

pub fn init() {
    // Kernel image end, page aligned.
    let kernel_end = (ekernel as usize + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    // Heap right after the kernel image.
    let after_heap = heap::init(kernel_end);
    // Frames after the heap.
    frame::init(after_heap);

    // Build the kernel page table and enable Sv39 paging.
    let pt = PageTable::new();
    map_kernel(&pt);
    KERNEL_PT.call_once(|| pt);

    unsafe {
        activate(pt.satp());
        // Allow S-mode to read/write U-mode pages (for syscall copyin/out).
        asm!("csrs sstatus, {}", in(reg) 1usize << 18); // SUM
    }
}

#[inline]
pub unsafe fn activate(satp: usize) {
    asm!("csrw satp, {}", in(reg) satp);
    asm!("sfence.vma");
}

pub fn kernel_pt() -> PageTable {
    *KERNEL_PT.get().unwrap()
}
