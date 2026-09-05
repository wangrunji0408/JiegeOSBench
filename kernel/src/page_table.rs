use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};
use crate::frame;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

#[inline]
fn pte_ppn(pa: usize) -> usize {
    (pa >> PAGE_SIZE_BITS) << 10
}

#[inline]
fn ppn_to_pa(pte: usize) -> usize {
    ((pte >> 10) & ((1 << 44) - 1)) << PAGE_SIZE_BITS
}

#[inline]
fn is_leaf(pte: usize) -> bool {
    pte & (PTE_R | PTE_W | PTE_X) != 0
}

/// A page table is identified by the physical address of its root frame.
/// Kernel memory is identity-mapped, so physical == virtual for table access.
#[derive(Clone, Copy)]
pub struct PageTable {
    pub root: usize,
}

impl PageTable {
    pub fn new() -> Self {
        let root = frame::alloc();
        PageTable { root }
    }

    fn table(pa: usize) -> &'static mut [usize] {
        unsafe { core::slice::from_raw_parts_mut(pa as *mut usize, 512) }
    }

    /// Map `va`->`pa` at the given level (0 = 4K, 1 = 2M, 2 = 1G) with flags.
    pub fn map_at(&self, va: usize, pa: usize, flags: usize, level: usize) {
        let idx = [
            (va >> 12) & 0x1ff,
            (va >> 21) & 0x1ff,
            (va >> 30) & 0x1ff,
        ];
        let mut t = self.root;
        let mut lvl = 2;
        while lvl > level {
            let pte = &mut Self::table(t)[idx[lvl]];
            if *pte & PTE_V == 0 {
                let np = frame::alloc();
                *pte = pte_ppn(np) | PTE_V;
            }
            t = ppn_to_pa(*pte);
            lvl -= 1;
        }
        let pte = &mut Self::table(t)[idx[level]];
        *pte = pte_ppn(pa) | flags | PTE_V | PTE_A | PTE_D;
    }

    pub fn map(&self, va: usize, pa: usize, flags: usize) {
        self.map_at(va, pa, flags, 0);
    }

    /// Remove a 4K mapping if present, returning the physical frame it pointed to.
    pub fn unmap(&self, va: usize) -> Option<usize> {
        let idx = [
            (va >> 12) & 0x1ff,
            (va >> 21) & 0x1ff,
            (va >> 30) & 0x1ff,
        ];
        let mut t = self.root;
        let mut lvl = 2;
        while lvl > 0 {
            let pte = Self::table(t)[idx[lvl]];
            if pte & PTE_V == 0 {
                return None;
            }
            t = ppn_to_pa(pte);
            lvl -= 1;
        }
        let pte = &mut Self::table(t)[idx[0]];
        if *pte & PTE_V == 0 {
            return None;
        }
        let pa = ppn_to_pa(*pte);
        *pte = 0;
        Some(pa)
    }

    /// Translate a user virtual address to physical, walking 4K/2M/1G leaves.
    pub fn translate(&self, va: usize) -> Option<usize> {
        let idx = [
            (va >> 12) & 0x1ff,
            (va >> 21) & 0x1ff,
            (va >> 30) & 0x1ff,
        ];
        let mut t = self.root;
        let mut lvl = 2;
        loop {
            let pte = Self::table(t)[idx[lvl]];
            if pte & PTE_V == 0 {
                return None;
            }
            if is_leaf(pte) {
                let base = ppn_to_pa(pte);
                let mask = (1usize << (12 + 9 * lvl)) - 1;
                return Some(base | (va & mask));
            }
            t = ppn_to_pa(pte);
            if lvl == 0 {
                return None;
            }
            lvl -= 1;
        }
    }

    pub fn satp(&self) -> usize {
        (8usize << 60) | (self.root >> PAGE_SIZE_BITS)
    }

    pub fn map_range(&self, va: usize, pa: usize, size: usize, flags: usize) {
        let mut off = 0;
        while off < size {
            self.map(va + off, pa + off, flags);
            off += PAGE_SIZE;
        }
    }
}
