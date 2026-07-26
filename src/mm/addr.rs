//! Address types and constants.
//!
//! Memory layout: the kernel identity-maps all of physical memory plus the MMIO
//! ranges into the low part of the address space. User space lives strictly
//! above `USER_BASE` so that a single page table can hold both, and the kernel
//! can dereference user pointers directly with `sstatus.SUM = 1`.

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// Physical memory base in the QEMU `virt` machine.
pub const MEMORY_START: usize = 0x8000_0000;
/// Total guest RAM we assume (must match the `-m` flag passed to QEMU).
pub const MEMORY_SIZE: usize = 1024 * 1024 * 1024;
pub const MEMORY_END: usize = MEMORY_START + MEMORY_SIZE;

/// Everything below this is kernel/identity space; user mappings start here.
/// Chosen above the identity region (RAM ends at 0xC000_0000) and low enough to
/// stay inside Sv39's 512 GiB span.
pub const USER_BASE: usize = 0x1_0000_0000;

/// Where a PIE executable's first segment is loaded.
pub const USER_ELF_BASE: usize = 0x2_0000_0000;
/// Where the dynamic linker (PT_INTERP) is loaded.
pub const USER_INTERP_BASE: usize = 0x3_0000_0000;
/// Base of the mmap region; allocations grow upward from here.
pub const USER_MMAP_BASE: usize = 0x4_0000_0000;
pub const USER_MMAP_TOP: usize = 0x7_0000_0000;
/// Top of the user stack. Stacks grow down from here, one per thread.
pub const USER_STACK_TOP: usize = 0x7_F000_0000;
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;
/// Highest legal user address.
pub const USER_TOP: usize = 0x8_0000_0000;

/// Kernel stack size for each task (used when trapping into the kernel).
pub const KERNEL_STACK_SIZE: usize = 256 * 1024;

#[inline]
pub const fn page_down(addr: usize) -> usize {
    addr & !PAGE_MASK
}

#[inline]
pub const fn page_up(addr: usize) -> usize {
    (addr + PAGE_MASK) & !PAGE_MASK
}

#[inline]
pub const fn is_page_aligned(addr: usize) -> bool {
    addr & PAGE_MASK == 0
}

/// True if `addr` is a plausible user-space address.
#[inline]
pub const fn is_user_addr(addr: usize) -> bool {
    addr >= USER_BASE && addr < USER_TOP
}

/// Convert a physical address to the kernel virtual address that maps it.
/// Identity mapping makes this the identity function, but going through these
/// helpers documents intent and leaves room to relocate the kernel later.
#[inline]
pub const fn phys_to_virt(pa: usize) -> usize {
    pa
}

#[inline]
pub const fn virt_to_phys(va: usize) -> usize {
    va
}

/// Borrow a physical frame as a byte slice.
///
/// # Safety
/// `pa` must name `len` bytes of live physical memory that no one else is
/// mutating for the lifetime of the returned slice.
#[inline]
pub unsafe fn phys_slice(pa: usize, len: usize) -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(phys_to_virt(pa) as *mut u8, len)
}
