//! Sv39 paging: 4 KiB pages and 2 MiB huge pages.

use core::arch::asm;

use super::frame;

pub const PAGE_SIZE: usize = 4096;
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

pub fn pte(ppn: usize, flags: usize) -> usize {
    (ppn << 10) | flags | PTE_V
}

#[derive(Clone, Copy)]
pub struct PageTable {
    pub root: usize, // physical address of root page table page
}

impl PageTable {
    pub fn new() -> Option<PageTable> {
        let root = frame::alloc_frame()?;
        unsafe {
            core::ptr::write_bytes(root as *mut u8, 0, PAGE_SIZE);
        }
        Some(PageTable { root })
    }

    pub fn get_pte(&self, vaddr: usize) -> Option<usize> {
        let (l2, l1, l0) = vpn(vaddr);
        unsafe {
            let t2 = self.root as *const usize;
            let pte2 = *t2.add(l2);
            if pte2 & PTE_V == 0 {
                return None;
            }
            if is_leaf(pte2) {
                return Some(pte2);
            }
            let t1 = ((pte2 >> 10) << 12) as *const usize;
            let pte1 = *t1.add(l1);
            if pte1 & PTE_V == 0 {
                return None;
            }
            if is_leaf(pte1) {
                return Some(pte1);
            }
            let t0 = ((pte1 >> 10) << 12) as *const usize;
            let pte0 = *t0.add(l0);
            if pte0 & PTE_V == 0 {
                return None;
            }
            Some(pte0)
        }
    }

    /// Translate a virtual address to physical (walks any leaf level).
    pub fn translate(&self, vaddr: usize) -> Option<usize> {
        let pte = self.get_pte(vaddr)?;
        let ppn = pte >> 10;
        // determine page size: if pte1 leaf -> 2MB; if pte2 leaf -> 1GB (not used)
        let (l2, l1, _l0) = vpn(vaddr);
        unsafe {
            let t2 = self.root as *const usize;
            let pte2 = *t2.add(l2);
            if is_leaf(pte2) {
                return Some((ppn << 12) + (vaddr & 0x3fff_ffff));
            }
            let t1 = ((pte2 >> 10) << 12) as *const usize;
            let pte1 = *t1.add(l1);
            if is_leaf(pte1) {
                return Some((ppn << 12) + (vaddr & 0x1f_ffff));
            }
            Some((ppn << 12) + (vaddr & 0xfff))
        }
    }

    pub fn map(&mut self, vaddr: usize, paddr: usize, flags: usize) {
        let (l2, l1, l0) = vpn(vaddr);
        unsafe {
            let t2 = self.root as *mut usize;
            let pte2 = *t2.add(l2);
            let mut t1 = if pte2 & PTE_V != 0 && !is_leaf(pte2) {
                ((pte2 >> 10) << 12) as *mut usize
            } else {
                let f = frame::alloc_frame().expect("oom pgtbl");
                core::ptr::write_bytes(f as *mut u8, 0, PAGE_SIZE);
                *t2.add(l2) = pte(f >> 12, PTE_R | PTE_W | PTE_A | PTE_D);
                f as *mut usize
            };
            let pte1 = *t1.add(l1);
            let mut t0 = if pte1 & PTE_V != 0 && !is_leaf(pte1) {
                ((pte1 >> 10) << 12) as *mut usize
            } else {
                let f = frame::alloc_frame().expect("oom pgtbl");
                core::ptr::write_bytes(f as *mut u8, 0, PAGE_SIZE);
                *t1.add(l1) = pte(f >> 12, PTE_R | PTE_W | PTE_A | PTE_D);
                f as *mut usize
            };
            let p = pte(paddr >> 12, flags | PTE_A | PTE_D);
            *t0.add(l0) = p;
        }
        sfence();
    }

    /// Map a 2 MiB-aligned range using huge pages.
    pub fn map_2mb(&mut self, vaddr: usize, paddr: usize, flags: usize) {
        assert!(vaddr % HUGE_PAGE_SIZE == 0 && paddr % HUGE_PAGE_SIZE == 0);
        let (l2, l1, _) = vpn(vaddr);
        unsafe {
            let t2 = self.root as *mut usize;
            let pte2 = *t2.add(l2);
            let mut t1 = if pte2 & PTE_V != 0 && !is_leaf(pte2) {
                ((pte2 >> 10) << 12) as *mut usize
            } else {
                let f = frame::alloc_frame().expect("oom pgtbl");
                core::ptr::write_bytes(f as *mut u8, 0, PAGE_SIZE);
                *t2.add(l2) = pte(f >> 12, PTE_R | PTE_W | PTE_A | PTE_D);
                f as *mut usize
            };
            let p = pte(paddr >> 12, flags | PTE_A | PTE_D);
            *t1.add(l1) = p;
        }
        sfence();
    }

    pub fn unmap(&mut self, vaddr: usize) {
        let (l2, l1, l0) = vpn(vaddr);
        unsafe {
            let t2 = self.root as *const usize;
            let pte2 = *t2.add(l2);
            if pte2 & PTE_V == 0 || is_leaf(pte2) {
                return;
            }
            let t1 = ((pte2 >> 10) << 12) as *mut usize;
            let pte1 = *t1.add(l1);
            if pte1 & PTE_V == 0 {
                return;
            }
            if is_leaf(pte1) {
                *t1.add(l1) = 0;
                return;
            }
            let t0 = ((pte1 >> 10) << 12) as *mut usize;
            *t0.add(l0) = 0;
        }
        sfence();
    }

    pub fn root_ppn(&self) -> usize {
        self.root >> 12
    }
}

fn vpn(vaddr: usize) -> (usize, usize, usize) {
    (
        (vaddr >> 30) & 0x1ff,
        (vaddr >> 21) & 0x1ff,
        (vaddr >> 12) & 0x1ff,
    )
}

fn is_leaf(pte: usize) -> bool {
    pte & (PTE_R | PTE_W | PTE_X) != 0
}

pub fn sfence() {
    unsafe {
        asm!("sfence.vma zero, zero", options(nostack));
    }
}

pub fn write_satp(root_ppn: usize) {
    unsafe {
        asm!(
            "csrw satp, {0}",
            in(reg) (8usize << 60) | root_ppn,
            options(nostack)
        );
    }
    sfence();
}
