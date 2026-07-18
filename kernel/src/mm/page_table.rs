//! SV39 page table: 3-level, 512 entries per level, 4KiB pages.

use super::address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::frame_allocator::{frame_alloc, FrameTracker};
use alloc::vec::Vec;
use bitflags::bitflags;

bitflags! {
    #[derive(Copy, Clone, Debug)]
    pub struct PTEFlags: u8 {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

impl PageTableEntry {
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: (ppn.0 << 10) | flags.bits() as usize,
        }
    }
    pub fn empty() -> Self {
        Self { bits: 0 }
    }
    pub fn ppn(&self) -> PhysPageNum {
        PhysPageNum((self.bits >> 10) & ((1usize << 44) - 1))
    }
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits as u8)
    }
    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }
    pub fn is_leaf(&self) -> bool {
        self.flags().intersects(PTEFlags::R | PTEFlags::W | PTEFlags::X)
    }
}

/// Owns the frames backing its own multi-level page directories (not the
/// leaf-mapped data frames, which are owned by the `MemorySet`'s `MapArea`s
/// or explicit `FrameTracker`s held elsewhere).
pub struct PageTable {
    pub root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

impl PageTable {
    pub fn new() -> Self {
        let frame = frame_alloc().expect("no memory for page table root");
        let root_ppn = frame.ppn;
        Self {
            root_ppn,
            frames: alloc::vec![frame],
        }
    }

    /// Build a `PageTable` view over an already-existing root, without
    /// owning any frames (used by the kernel to peek at a user page table
    /// via its satp token without taking ownership).
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }

    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        for (i, &idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.as_pte_array()[idx];
            if i == 2 {
                return Some(pte);
            }
            if !pte.is_valid() {
                let frame = frame_alloc()?;
                *pte = PageTableEntry::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }
            ppn = pte.ppn();
        }
        unreachable!()
    }

    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        for (i, &idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.as_pte_array()[idx];
            if i == 2 {
                return Some(pte);
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        unreachable!()
    }

    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).expect("mapping failed: no memory");
        debug_assert!(!pte.is_valid(), "vpn {:#x} already mapped", vpn.0);
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }

    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).expect("unmapping invalid vpn");
        debug_assert!(pte.is_valid(), "vpn {:#x} not mapped", vpn.0);
        *pte = PageTableEntry::empty();
    }

    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }

    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        let vpn = va.floor();
        self.translate(vpn).map(|pte| {
            let aligned: PhysAddr = pte.ppn().into();
            PhysAddr(aligned.0 + va.page_offset())
        })
    }

    /// Update flags of an already-mapped page (used by mprotect).
    pub fn set_flags(&mut self, vpn: VirtPageNum, flags: PTEFlags) {
        let pte = self.find_pte(vpn).expect("set_flags on unmapped vpn");
        let ppn = pte.ppn();
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }
}
