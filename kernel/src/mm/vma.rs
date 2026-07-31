//! Per-process user memory management: VMA list + page mapping helpers.

use crate::mm::frame;
use crate::mm::paging::{self, PageTable};

pub const USER_STACK_TOP: usize = 0x0000_003f_0000_0000; // below Sv39 user limit 2^38
pub const USER_MMAP_BASE: usize = 0x0000_003e_0000_0000; // grows down
pub const USER_LIMIT: usize = 0x0000_0040_0000_0000;

#[derive(Clone, Copy, PartialEq)]
pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub prot: usize, // PTE_R/W/X bits (kernel flag namespace: R=2,W=4,X=8)
    pub anon: bool,
    pub file_id: Option<usize>, // fs file id for file-backed (unused for now)
}

pub const PROT_NONE: usize = 0;
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
pub const PROT_EXEC: usize = 4;

pub fn prot_to_pte(p: usize) -> usize {
    let mut f = 0;
    if p & PROT_READ != 0 {
        f |= paging::PTE_R;
    }
    if p & PROT_WRITE != 0 {
        f |= paging::PTE_W;
    }
    if p & PROT_EXEC != 0 {
        f |= paging::PTE_X;
    }
    f | paging::PTE_U
}

#[derive(Clone)]
pub struct Mm {
    pub pt: PageTable,
    pub vmas: Vec<Vma>,
    pub brk: usize,      // current program break
    pub brk_start: usize,
    pub mmap_next: usize,
    pub stack_top: usize,
}

impl Mm {
    pub fn new() -> Mm {
        let pt = PageTable::new().expect("oom mm pt");
        Mm {
            pt,
            vmas: Vec::new(),
            brk: 0,
            brk_start: 0,
            mmap_next: USER_MMAP_BASE,
            stack_top: USER_STACK_TOP,
        }
    }

    pub fn find_vma(&self, addr: usize) -> Option<&Vma> {
        self.vmas.iter().find(|v| addr >= v.start && addr < v.end)
    }

    /// Validate a user range [addr, addr+len) for the given access.
    pub fn check_range(&self, addr: usize, len: usize, write: bool) -> bool {
        if addr >= USER_LIMIT || len > USER_LIMIT || addr + len < addr || addr + len > USER_LIMIT {
            return false;
        }
        if len == 0 {
            return addr < USER_LIMIT;
        }
        let mut cur = addr;
        while cur < addr + len {
            match self.find_vma(cur) {
                Some(v) => {
                    if write && v.prot & PROT_WRITE == 0 {
                        return false;
                    }
                    if !write && v.prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0 {
                        return false;
                    }
                    cur = v.end;
                }
                None => return false,
            }
        }
        true
    }

    pub fn map_anon(&mut self, start: usize, end: usize, prot: usize) {
        assert!(start % paging::PAGE_SIZE == 0);
        for a in (start..end).step_by(paging::PAGE_SIZE) {
            let f = frame::alloc_frame().expect("oom anon");
            // zero it
            unsafe {
                core::ptr::write_bytes(f as *mut u8, 0, paging::PAGE_SIZE);
            }
            self.pt.map(a, f, prot_to_pte(prot));
        }
        self.vmas.push(Vma {
            start,
            end,
            prot,
            anon: true,
            file_id: None,
        });
        self.merge_vmas();
    }

    /// Map file-backed region by copying data (private mapping semantics).
    pub fn map_file(&mut self, start: usize, end: usize, prot: usize, data: &[u8], offset: usize) {
        for a in (start..end).step_by(paging::PAGE_SIZE) {
            let f = frame::alloc_frame().expect("oom file");
            unsafe {
                core::ptr::write_bytes(f as *mut u8, 0, paging::PAGE_SIZE);
            }
            let src = offset + (a - start);
            if src < data.len() {
                let n = core::cmp::min(paging::PAGE_SIZE, data.len() - src);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr().add(src),
                        f as *mut u8,
                        n,
                    );
                }
            }
            self.pt.map(a, f, prot_to_pte(prot));
        }
        self.vmas.push(Vma {
            start,
            end,
            prot,
            anon: false,
            file_id: None,
        });
        self.merge_vmas();
    }

    pub fn unmap_range(&mut self, start: usize, end: usize) {
        let mut a = start;
        while a < end {
            if let Some(f) = self.pt.translate(a) {
                // don't free kernel pages; user pages only (kernel is identity, not in vmas)
                frame::free_frame(f);
            }
            self.pt.unmap(a);
            a += paging::PAGE_SIZE;
        }
        self.vmas.retain(|v| v.end <= start || v.start >= end);
    }

    pub fn mprotect_range(&mut self, start: usize, end: usize, prot: usize) {
        for v in self.vmas.iter_mut() {
            if v.end > start && v.start < end {
                v.prot = prot;
                let s = core::cmp::max(v.start, start);
                let e = core::cmp::min(v.end, end);
                let mut a = s;
                while a < e {
                    if let Some(pte) = self.pt.get_pte(a) {
                        let ppn = pte >> 10;
                        let newp = (ppn << 10) | prot_to_pte(prot) | paging::PTE_A | paging::PTE_D;
                        // rewrite leaf pte (only for 4K pages)
                        let (l2, l1, l0) = (
                            (a >> 30) & 0x1ff,
                            (a >> 21) & 0x1ff,
                            (a >> 12) & 0x1ff,
                        );
                        unsafe {
                            let t2 = self.pt.root as *const usize;
                            let pte2 = *t2.add(l2);
                            let t1 = ((pte2 >> 10) << 12) as *const usize;
                            let pte1 = *t1.add(l1);
                            let t0 = ((pte1 >> 10) << 12) as *mut usize;
                            *t0.add(l0) = newp;
                        }
                    }
                    a += paging::PAGE_SIZE;
                }
            }
        }
        paging::sfence();
    }

    fn merge_vmas(&mut self) {
        self.vmas.sort_by_key(|v| v.start);
        let mut merged: Vec<Vma> = Vec::new();
        for v in self.vmas.drain(..) {
            if let Some(last) = merged.last_mut() {
                if last.end == v.start && last.prot == v.prot && last.anon == v.anon {
                    last.end = v.end;
                    continue;
                }
            }
            merged.push(v);
        }
        self.vmas = merged;
    }

    /// Copy the whole address space (for fork). Page-by-page copy.
    pub fn copy_from(&mut self, src: &Mm) {
        for v in &src.vmas {
            let mut a = v.start;
            while a < v.end {
                let f = frame::alloc_frame().expect("oom fork");
                if let Some(phys) = src.pt.translate(a) {
                    unsafe {
                        core::ptr::copy_nonoverlapping(phys as *const u8, f as *mut u8, paging::PAGE_SIZE);
                    }
                } else {
                    unsafe {
                        core::ptr::write_bytes(f as *mut u8, 0, paging::PAGE_SIZE);
                    }
                }
                self.pt.map(a, f, prot_to_pte(v.prot));
                a += paging::PAGE_SIZE;
            }
            self.vmas.push(Vma { start: v.start, end: v.end, prot: v.prot, anon: v.anon, file_id: v.file_id });
        }
        self.brk = src.brk;
        self.brk_start = src.brk_start;
        self.mmap_next = src.mmap_next;
        self.stack_top = src.stack_top;
        // kernel mappings must be copied too: identity map of RAM+MMIO
        crate::mm::map_kernel_into(&mut self.pt);
    }

    /// Free all user memory.
    pub fn destroy(&mut self) {
        for v in self.vmas.clone() {
            self.unmap_range(v.start, v.end);
        }
        // free page table pages (recursive) — walk all levels
        self.free_page_tables(self.pt.root, 2);
    }

    fn free_page_tables(&self, table: usize, level: usize) {
        if level == 0 {
            frame::free_frame(table);
            return;
        }
        let entries = unsafe { core::slice::from_raw_parts(table as *const usize, 512) };
        for &e in entries {
            if e & paging::PTE_V == 0 {
                continue;
            }
            if e & (paging::PTE_R | paging::PTE_W | paging::PTE_X) != 0 {
                continue; // leaf
            }
            self.free_page_tables((e >> 10) << 12, level - 1);
        }
        frame::free_frame(table);
    }
}
