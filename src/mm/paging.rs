//! Sv39 page tables.
//!
//! Address space layout (per process):
//!   root[0]  0x0000_0000..0x4000_0000  shared MMIO identity map (kernel-only)
//!   root[1]  0x4000_0000..0x8000_0000  user space (ELF, heap, mmap, stack)
//!   root[2]  0x8000_0000..0xC000_0000  kernel RAM identity map (1G page, global)
use super::{frame, PAGE_SIZE};
use core::arch::asm;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

const fn pte(pa: usize, flags: usize) -> usize {
    (pa >> 12) << 10 | flags
}
const fn pte_pa(e: usize) -> usize {
    (e >> 10) << 12
}

/// Shared level-1 table mapping MMIO (built once).
static mut MMIO_L1: usize = 0;

fn mmio_l1() -> usize {
    unsafe {
        if MMIO_L1 == 0 {
            let l1 = frame::alloc();
            let t = l1 as *mut usize;
            // 2M page covering 0x1000_0000..0x1020_0000: UART0 + all virtio-mmio slots
            let idx = (0x1000_0000usize >> 21) & 0x1ff;
            *t.add(idx) = pte(0x1000_0000, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D | PTE_G);
            MMIO_L1 = l1;
        }
        MMIO_L1
    }
}

pub struct PageTable {
    pub root: usize,
}

impl PageTable {
    /// New page table with kernel mappings (MMIO + RAM identity).
    pub fn new() -> Self {
        let root = frame::alloc();
        let t = root as *mut usize;
        unsafe {
            *t.add(0) = pte(mmio_l1(), PTE_V); // non-leaf
            *t.add(2) = pte(
                0x8000_0000,
                PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D | PTE_G,
            );
        }
        PageTable { root }
    }

    fn walk_alloc(&mut self, va: usize) -> *mut usize {
        let mut table = self.root as *mut usize;
        for level in (1..3).rev() {
            let idx = (va >> (12 + 9 * level)) & 0x1ff;
            let e = unsafe { *table.add(idx) };
            let next = if e & PTE_V == 0 {
                let f = frame::alloc();
                unsafe { *table.add(idx) = pte(f, PTE_V) };
                f
            } else {
                assert!(e & (PTE_R | PTE_W | PTE_X) == 0, "remap over huge page va={va:#x}");
                pte_pa(e)
            };
            table = next as *mut usize;
        }
        unsafe { table.add((va >> 12) & 0x1ff) }
    }

    fn walk(&self, va: usize) -> Option<*mut usize> {
        let mut table = self.root as *mut usize;
        for level in (1..3).rev() {
            let idx = (va >> (12 + 9 * level)) & 0x1ff;
            let e = unsafe { *table.add(idx) };
            if e & PTE_V == 0 {
                return None;
            }
            if e & (PTE_R | PTE_W | PTE_X) != 0 {
                return None; // huge page — not used for user mappings
            }
            table = pte_pa(e) as *mut usize;
        }
        Some(unsafe { table.add((va >> 12) & 0x1ff) })
    }

    /// Map one 4K page. Overwrites any existing mapping.
    pub fn map(&mut self, va: usize, pa: usize, flags: usize) {
        let e = self.walk_alloc(va);
        unsafe { *e = pte(pa, flags | PTE_V | PTE_A | PTE_D) };
    }

    /// Unmap one page, returning the old physical address if it was mapped.
    pub fn unmap(&mut self, va: usize) -> Option<usize> {
        let e = self.walk(va)?;
        let old = unsafe { *e };
        if old & PTE_V == 0 {
            return None;
        }
        unsafe { *e = 0 };
        Some(pte_pa(old))
    }

    pub fn translate(&self, va: usize) -> Option<(usize, usize)> {
        let e = self.walk(va)?;
        let v = unsafe { *e };
        if v & PTE_V == 0 {
            return None;
        }
        Some((pte_pa(v) + (va & 0xfff), v & 0x3ff))
    }

    /// Update flags of an existing mapping (for mprotect).
    pub fn protect(&mut self, va: usize, flags: usize) -> bool {
        if let Some(e) = self.walk(va) {
            let v = unsafe { *e };
            if v & PTE_V != 0 {
                unsafe { *e = (v & !0x3ffusize) | flags | PTE_V | PTE_A | PTE_D };
                return true;
            }
        }
        false
    }

    pub fn satp(&self) -> usize {
        8usize << 60 | (self.root >> 12)
    }

    pub unsafe fn activate(&self) {
        asm!("csrw satp, {}", "sfence.vma", in(reg) self.satp());
    }
}

pub fn flush_tlb() {
    unsafe { asm!("sfence.vma") };
}

/// Copy user pages (for keeping things simple we don't refcount; used by frame freeing on munmap).
pub const USER_FLAGS_RWX: usize = PTE_U | PTE_R | PTE_W | PTE_X;

/// Convert Linux PROT_* to PTE flags (always readable if mapped).
pub fn prot_to_flags(prot: usize) -> usize {
    let mut f = PTE_U | PTE_R;
    if prot & 2 != 0 {
        f |= PTE_W;
    }
    if prot & 4 != 0 {
        f |= PTE_X;
    }
    f
}
