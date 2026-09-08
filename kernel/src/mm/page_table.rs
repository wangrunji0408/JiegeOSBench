//! Sv39 page tables (4 KiB and 2 MiB leaf mappings).

use crate::mm::address::*;
use crate::mm::frame;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

pub const PPN_MASK: usize = (1usize << 44) - 1;

#[inline]
fn pte_ppn(pte: usize) -> usize {
    (pte >> 10) & PPN_MASK
}

#[inline]
fn pa_to_pte(pa: usize, flags: usize) -> usize {
    ((pa >> 12) << 10) | flags
}

#[derive(Clone, Copy)]
pub struct PageTable {
    pub root: usize,
}

unsafe impl Send for PageTable {}

fn table_ptr(pa: usize) -> *mut usize {
    pa as *mut usize
}

impl PageTable {
    /// A fresh, empty page table (root frame zeroed).
    pub fn new() -> Self {
        let root = frame::alloc_frame().expect("out of frames for page table");
        Self { root }
    }

    /// Build a user page table that shares the kernel's mappings (Sv39 entries
    /// >= 2), so the kernel stays mapped while user code runs.
    pub fn new_user(kernel_root: usize) -> Self {
        let root = frame::alloc_frame().expect("out of frames for page table");
        unsafe {
            let dst = table_ptr(root);
            let src = table_ptr(kernel_root);
            // Copy all kernel-level entries (user space lives in entries 0..2).
            for i in 2..512 {
                let e = src.add(i).read();
                if e & PTE_V != 0 {
                    dst.add(i).write(e);
                }
            }
        }
        Self { root }
    }

    pub fn root(&self) -> usize {
        self.root
    }

    /// satp value with this page table active.
    pub fn satp(&self) -> usize {
        (8usize << 60) | (self.root >> 12)
    }

    fn next_table(&mut self, table_pa: usize, idx: usize, alloc: bool) -> Option<usize> {
        unsafe {
            let p = table_ptr(table_pa);
            let e = p.add(idx).read();
            if e & PTE_V != 0 {
                if e & (PTE_R | PTE_W | PTE_X) != 0 {
                    // a leaf in the middle: cannot descend
                    return None;
                }
                Some(pte_ppn(e) << 12)
            } else if alloc {
                let f = frame::alloc_frame()?;
                p.add(idx).write(pa_to_pte(f, PTE_V));
                Some(f)
            } else {
                None
            }
        }
    }

    /// Map a single 4 KiB page.
    pub fn map(&mut self, va: usize, pa: usize, flags: usize) -> Result<(), ()> {
        debug_assert!(is_page_aligned(va) && is_page_aligned(pa));
        let idx0 = (va >> 12) & 0x1ff;
        let idx1 = (va >> 21) & 0x1ff;
        let idx2 = (va >> 30) & 0x1ff;
        let t1 = self.next_table(self.root, idx2, true).ok_or(())?;
        let t0 = self.next_table(t1, idx1, true).ok_or(())?;
        unsafe {
            let p = table_ptr(t0);
            let old = p.add(idx0).read();
            if old & PTE_V != 0 && old & (PTE_R | PTE_W | PTE_X) != 0 {
                return Err(());
            }
            p.add(idx0).write(pa_to_pte(pa, flags | PTE_V | PTE_A | PTE_D));
        }
        Ok(())
    }

    /// Map a 2 MiB page (va and pa must be 2 MiB aligned).
    pub fn map_2m(&mut self, va: usize, pa: usize, flags: usize) -> Result<(), ()> {
        debug_assert!(va & 0x1f_ffff == 0 && pa & 0x1f_ffff == 0);
        let idx1 = (va >> 21) & 0x1ff;
        let idx2 = (va >> 30) & 0x1ff;
        let t1 = self.next_table(self.root, idx2, true).ok_or(())?;
        unsafe {
            let p = table_ptr(t1);
            p.add(idx1)
                .write(pa_to_pte(pa, flags | PTE_V | PTE_A | PTE_D));
        }
        Ok(())
    }

    /// Map a range of physical memory starting at `va` with 4 KiB pages.
    pub fn map_range(&mut self, va: usize, pa: usize, size: usize, flags: usize) -> Result<(), ()> {
        let mut off = 0;
        while off < size {
            self.map(va + off, pa + off, flags)?;
            off += PAGE_SIZE;
        }
        Ok(())
    }

    pub fn unmap(&mut self, va: usize) -> Option<usize> {
        let idx0 = (va >> 12) & 0x1ff;
        let idx1 = (va >> 21) & 0x1ff;
        let idx2 = (va >> 30) & 0x1ff;
        let t1 = self.next_table(self.root, idx2, false)?;
        let t0 = self.next_table(t1, idx1, false)?;
        unsafe {
            let p = table_ptr(t0);
            let old = p.add(idx0).read();
            if old & PTE_V == 0 {
                return None;
            }
            p.add(idx0).write(0);
            Some(pte_ppn(old) << 12)
        }
    }

    /// Translate a virtual address to (physical, pte flags).
    pub fn translate(&self, va: usize) -> Option<(usize, usize)> {
        let idx0 = (va >> 12) & 0x1ff;
        let idx1 = (va >> 21) & 0x1ff;
        let idx2 = (va >> 30) & 0x1ff;
        unsafe {
            let p2 = table_ptr(self.root);
            let e2 = p2.add(idx2).read();
            if e2 & PTE_V == 0 {
                return None;
            }
            if e2 & (PTE_R | PTE_W | PTE_X) != 0 {
                // 1 GiB leaf
                let base = pte_ppn(e2) << 12;
                return Some((base + (va & ((1 << 30) - 1)), e2));
            }
            let p1 = table_ptr(pte_ppn(e2) << 12);
            let e1 = p1.add(idx1).read();
            if e1 & PTE_V == 0 {
                return None;
            }
            if e1 & (PTE_R | PTE_W | PTE_X) != 0 {
                // 2 MiB leaf
                let base = pte_ppn(e1) << 12;
                return Some((base + (va & ((1 << 21) - 1)), e1));
            }
            let p0 = table_ptr(pte_ppn(e1) << 12);
            let e0 = p0.add(idx0).read();
            if e0 & PTE_V == 0 || e0 & (PTE_R | PTE_W | PTE_X) == 0 {
                return None;
            }
            let base = pte_ppn(e0) << 12;
            Some((base + (va & (PAGE_SIZE - 1)), e0))
        }
    }

    /// Translate a *user* address: must be present and have the U bit.
    pub fn translate_user(&self, va: usize) -> Option<usize> {
        self.translate(va).and_then(|(pa, flags)| {
            if flags & PTE_U != 0 {
                Some(pa)
            } else {
                None
            }
        })
    }

    pub fn set_flags(&mut self, va: usize, flags: usize) -> bool {
        let idx0 = (va >> 12) & 0x1ff;
        let idx1 = (va >> 21) & 0x1ff;
        let idx2 = (va >> 30) & 0x1ff;
        unsafe {
            let p2 = table_ptr(self.root);
            let e2 = p2.add(idx2).read();
            if e2 & PTE_V == 0 || e2 & (PTE_R | PTE_W | PTE_X) != 0 {
                return false;
            }
            let p1 = table_ptr(pte_ppn(e2) << 12);
            let e1 = p1.add(idx1).read();
            if e1 & PTE_V == 0 || e1 & (PTE_R | PTE_W | PTE_X) != 0 {
                return false;
            }
            let p0 = table_ptr(pte_ppn(e1) << 12);
            let e0 = p0.add(idx0).read();
            if e0 & PTE_V == 0 {
                return false;
            }
            p0.add(idx0)
                .write((e0 & !0x3ff) | flags | PTE_V | PTE_A | PTE_D);
        }
        true
    }

    /// Free every level-0/1/2 table reachable from this root (user pages are
    /// freed separately by the caller).
    pub fn free_tables(&mut self) {
        unsafe {
            let p2 = table_ptr(self.root);
            for i in 0..2 {
                let e2 = p2.add(i).read();
                if e2 & PTE_V == 0 || e2 & (PTE_R | PTE_W | PTE_X) != 0 {
                    continue;
                }
                let p1 = table_ptr(pte_ppn(e2) << 12);
                for j in 0..512 {
                    let e1 = p1.add(j).read();
                    if e1 & PTE_V == 0 || e1 & (PTE_R | PTE_W | PTE_X) != 0 {
                        continue;
                    }
                    frame::free_frame(pte_ppn(e1) << 12);
                }
                frame::free_frame(pte_ppn(e2) << 12);
            }
        }
    }
}

/// Build the kernel page table: identity-map RAM with 2 MiB pages and map the
/// MMIO devices in the high half.
pub fn init_kernel(root: usize, ram_start: usize, ram_end: usize, initrd: Option<(usize, usize)>) {
    let mut pt = PageTable { root };
    // identity map RAM
    let start = page_align_down(ram_start);
    let end = page_align_up(ram_end);
    let mut pa = start;
    while pa < end {
        pt.map_2m(pa, pa, PTE_R | PTE_W | PTE_X).unwrap();
        pa += 1 << 21;
    }
    // devices in the high half (UART, PLIC, CLINT, virtio-mmio)
    for &(base, size) in &[
        (0x0c00_0000usize, 0x40_0000usize), // PLIC
        (0x1000_0000, 0x10_0000),           // UART + RTC + virtio-mmio
        (0x0200_0000, 0x1_0000),            // CLINT (not used from S-mode but handy)
    ] {
        let mut off = 0;
        while off < size {
            pt.map(dev_addr(base + off), base + off, PTE_R | PTE_W)
                .unwrap();
            off += PAGE_SIZE;
        }
    }
    let _ = initrd;
}
