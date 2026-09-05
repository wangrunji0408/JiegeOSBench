//! Sv39 page tables.
use alloc::vec::Vec;
use bitflags::bitflags;

use super::frame::Frame;
use crate::config::{PAGE_SIZE, RAM_END, RAM_START};

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct PteFlags: usize {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
        // software bits
        const COW = 1 << 8;
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Pte(pub usize);

impl Pte {
    pub const fn empty() -> Self {
        Pte(0)
    }
    pub fn new(pa: usize, flags: PteFlags) -> Self {
        Pte(((pa >> 12) << 10) | flags.bits())
    }
    pub fn pa(&self) -> usize {
        (self.0 >> 10) << 12
    }
    pub fn flags(&self) -> PteFlags {
        PteFlags::from_bits_truncate(self.0 & 0x3ff)
    }
    pub fn is_valid(&self) -> bool {
        self.0 & 1 != 0
    }
    pub fn is_leaf(&self) -> bool {
        self.is_valid() && (self.0 & 0xe) != 0
    }
}

fn table_of(pa: usize) -> &'static mut [Pte; 512] {
    unsafe { &mut *(pa as *mut [Pte; 512]) }
}

#[inline]
fn vpn(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}

pub struct PageTable {
    root: Frame,
    tables: Vec<Frame>,
}

impl PageTable {
    /// Create a new page table containing the kernel mappings.
    pub fn new_kernel() -> Self {
        let mut pt = PageTable { root: Frame::alloc(), tables: Vec::new() };
        let root = table_of(pt.root.pa());
        // 1 GiB huge pages covering RAM (supervisor RWX).
        let kflags = PteFlags::V | PteFlags::R | PteFlags::W | PteFlags::X | PteFlags::A | PteFlags::D | PteFlags::G;
        let mut gb = RAM_START & !((1 << 30) - 1);
        while gb < RAM_END {
            root[vpn(gb, 2)] = Pte::new(gb, kflags);
            gb += 1 << 30;
        }
        // MMIO: 2 MiB leaves in region 0 for PLIC (0x0c00_0000..0x0c60_0000) and
        // UART/virtio (0x1000_0000..0x1020_0000). The goldfish RTC at 0x10_1000 is
        // only touched during boot (paging off) so it stays unmapped here, leaving
        // low addresses free for non-PIE user executables.
        let l1 = Frame::alloc();
        root[0] = Pte::new(l1.pa(), PteFlags::V);
        let l1t = table_of(l1.pa());
        let io = PteFlags::V | PteFlags::R | PteFlags::W | PteFlags::A | PteFlags::D | PteFlags::G;
        for base in [0x0c00_0000usize, 0x0c20_0000, 0x0c40_0000, 0x1000_0000] {
            l1t[vpn(base, 1)] = Pte::new(base, io);
        }
        pt.tables.push(l1);
        pt
    }

    pub fn satp(&self) -> usize {
        (8 << 60) | (self.root.pa() >> 12)
    }

    pub fn root_pa(&self) -> usize {
        self.root.pa()
    }

    /// Walk to the level-0 PTE for `va`, creating intermediate tables if `create`.
    fn walk(&mut self, va: usize, create: bool) -> Option<&mut Pte> {
        let mut table = table_of(self.root.pa());
        for level in (1..3).rev() {
            let pte = &mut table[vpn(va, level)];
            if !pte.is_valid() {
                if !create {
                    return None;
                }
                let f = Frame::alloc();
                *pte = Pte::new(f.pa(), PteFlags::V);
                self.tables.push(f);
            } else if pte.is_leaf() {
                // huge page (kernel region) – not splittable
                return None;
            }
            table = table_of(pte.pa());
        }
        Some(&mut table[vpn(va, 0)])
    }

    fn walk_ro(&self, va: usize) -> Option<Pte> {
        let mut table = table_of(self.root.pa());
        for level in (1..3).rev() {
            let pte = table[vpn(va, level)];
            if !pte.is_valid() || pte.is_leaf() {
                return None;
            }
            table = table_of(pte.pa());
        }
        Some(table[vpn(va, 0)])
    }

    pub fn map(&mut self, va: usize, pa: usize, flags: PteFlags) {
        debug_assert!(va % PAGE_SIZE == 0 && pa % PAGE_SIZE == 0);
        let pte = self.walk(va, true).expect("map: huge page conflict");
        *pte = Pte::new(pa, flags | PteFlags::V | PteFlags::A | PteFlags::D);
    }

    /// Remove mapping; returns the old PTE if one existed.
    pub fn unmap(&mut self, va: usize) -> Option<Pte> {
        let pte = self.walk(va, false)?;
        if pte.is_valid() {
            let old = *pte;
            *pte = Pte::empty();
            Some(old)
        } else {
            None
        }
    }

    pub fn get(&self, va: usize) -> Option<Pte> {
        let pte = self.walk_ro(va)?;
        if pte.is_valid() {
            Some(pte)
        } else {
            None
        }
    }

    pub fn set_flags(&mut self, va: usize, flags: PteFlags) -> bool {
        if let Some(pte) = self.walk(va, false) {
            if pte.is_valid() {
                *pte = Pte::new(pte.pa(), flags | PteFlags::V | PteFlags::A | PteFlags::D);
                return true;
            }
        }
        false
    }

    /// Translate a user virtual address to a physical address (must be mapped).
    pub fn translate(&self, va: usize) -> Option<(usize, PteFlags)> {
        let pte = self.get(va)?;
        Some((pte.pa() + (va & (PAGE_SIZE - 1)), pte.flags()))
    }
}

#[inline]
pub fn flush_tlb() {
    unsafe { core::arch::asm!("sfence.vma") };
}

#[inline]
pub fn flush_tlb_page(va: usize) {
    unsafe { core::arch::asm!("sfence.vma {}, zero", in(reg) va) };
}
