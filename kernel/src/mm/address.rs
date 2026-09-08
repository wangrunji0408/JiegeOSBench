//! Physical/virtual address helpers and the memory map.

pub type PhysAddr = usize;
pub type VirtAddr = usize;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

/// Kernel image link base (QEMU virt: RAM starts at 0x8000_0000, OpenSBI jumps
/// to 0x8020_0000 in S-mode).
pub const KERNEL_LINK_BASE: usize = 0x8020_0000;

/// RAM base of the QEMU virt machine.
pub const RAM_BASE: usize = 0x8000_0000;

/// Devices are identity-mapped in the kernel page table, but because the user
/// address space occupies Sv39 entries 0..2 we expose them through this high
/// half alias instead (`dev_addr(pa) = pa + DEV_BASE`).
pub const DEV_BASE: usize = 0xFFFF_FFC0_0000_0000;

#[inline(always)]
pub const fn dev_addr(pa: usize) -> usize {
    pa.wrapping_add(DEV_BASE)
}

#[inline(always)]
pub const fn dev_ptr<T>(pa: usize) -> *mut T {
    dev_addr(pa) as *mut T
}

#[inline(always)]
pub const fn page_align_down(x: usize) -> usize {
    x & !(PAGE_SIZE - 1)
}

#[inline(always)]
pub const fn page_align_up(x: usize) -> usize {
    (x + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

#[inline(always)]
pub const fn is_page_aligned(x: usize) -> bool {
    x & (PAGE_SIZE - 1) == 0
}
