//! RISC-V Sv39 page table.

use super::frame::{self, PAGE_SIZE};

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

/// Flags for a read/write/execute kernel page.
pub const KERNEL_RWX: usize = PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
/// Flags for a read/write (device) kernel page.
pub const KERNEL_RW: usize = PTE_R | PTE_W | PTE_A | PTE_D;
/// Flags for a read/write/execute user page.
pub const USER_RWX: usize = PTE_U | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
/// Flags for a read/write user page.
pub const USER_RW: usize = PTE_U | PTE_R | PTE_W | PTE_A | PTE_D;
/// Flags for a read-only user page.
pub const USER_R: usize = PTE_U | PTE_R | PTE_A | PTE_D;

pub struct PageTable {
    root: usize, // physical address of the root table
}

impl PageTable {
    pub fn new() -> Self {
        let f = frame::alloc().expect("out of frames for page table");
        Self { root: f.0 }
    }

    pub fn root(&self) -> usize {
        self.root
    }

    /// Sv39 satp value (mode 8).
    pub fn satp(&self) -> usize {
        (8usize << 60) | (self.root >> 12)
    }

    /// Walk the page table to the leaf PTE slot for `va`.
    /// Allocates intermediate tables when `create` is true.
    fn walk(&self, va: usize, create: bool) -> Option<*mut usize> {
        let mut table = self.root;
        for level in (1..=2).rev() {
            let idx = (va >> (12 + 9 * level)) & 0x1ff;
            let pte_ptr = (table + idx * 8) as *mut usize;
            let pte = unsafe { *pte_ptr };
            if pte & PTE_V == 0 {
                if !create {
                    return None;
                }
                let f = frame::alloc().expect("out of frames for page table level");
                unsafe { *pte_ptr = ((f.0 >> 12) << 10) | PTE_V };
                table = f.0;
            } else {
                table = (pte >> 10) << 12;
            }
        }
        let idx = (va >> 12) & 0x1ff;
        Some((table + idx * 8) as *mut usize)
    }

    pub fn map(&mut self, va: usize, pa: usize, flags: usize) {
        assert!(va & (PAGE_SIZE - 1) == 0, "unaligned va {:#x}", va);
        assert!(pa & (PAGE_SIZE - 1) == 0, "unaligned pa {:#x}", pa);
        let pte = self.walk(va, true).unwrap();
        unsafe { *pte = ((pa >> 12) << 10) | flags | PTE_V };
    }

    /// Map an arbitrary (possibly non-page-aligned) range to physical memory,
    /// using page granularity.
    pub fn map_range(&mut self, va_start: usize, pa_start: usize, len: usize, flags: usize) {
        let start = frame::align_down(va_start, PAGE_SIZE);
        let end = frame::align_up(va_start + len, PAGE_SIZE);
        let mut va = start;
        let mut pa = frame::align_down(pa_start, PAGE_SIZE);
        while va < end {
            self.map(va, pa, flags);
            va += PAGE_SIZE;
            pa += PAGE_SIZE;
        }
    }

    pub fn unmap(&mut self, va: usize) {
        if let Some(pte) = self.walk(va, false) {
            unsafe { *pte = 0 };
        }
    }

    pub fn translate(&self, va: usize) -> Option<usize> {
        let pte = self.walk(va, false)?;
        let pte_val = unsafe { *pte };
        if pte_val & PTE_V == 0 {
            return None;
        }
        Some(((pte_val >> 10) << 12) | (va & 0xfff))
    }
}

/// Build the kernel page table: identity-map RAM and MMIO regions.
pub fn kernel_page_table() -> PageTable {
    let mut pt = PageTable::new();

    // Identity-map all of RAM (kernel code/data/heap/frames live here).
    let mut va = crate::memory::MEMORY_START;
    while va < crate::memory::MEMORY_END {
        pt.map(va, va, KERNEL_RWX);
        va += PAGE_SIZE;
    }

    // Identity-map MMIO regions.
    for &(start, end) in crate::memory::MMIO_REGIONS {
        let mut va = start;
        while va < end {
            pt.map(va, va, KERNEL_RW);
            va += PAGE_SIZE;
        }
    }

    pt
}
