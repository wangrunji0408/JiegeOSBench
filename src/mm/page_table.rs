//! Sv39 页表

use crate::mm::addr::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use crate::mm::frame::{frame_alloc, FrameTracker};
use alloc::vec::Vec;
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy)]
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
        PhysPageNum((self.bits >> 10) & ((1 << 44) - 1))
    }
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.bits as u8)
    }
    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }
    pub fn set_flags(&mut self, flags: PTEFlags) {
        self.bits = (self.bits & !0xff) | flags.bits() as usize;
    }
}

pub struct PageTable {
    pub root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>, // 持有页表页，防止被释放
}

impl PageTable {
    pub fn new() -> Self {
        let frame = frame_alloc().expect("no frame for page table");
        Self {
            root_ppn: frame.ppn,
            frames: alloc::vec![frame],
        }
    }

    /// 临时借用一个根页表（不持有帧），用于 fork 时访问
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum(satp & ((1 << 44) - 1)),
            frames: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        (8 << 60) | self.root_ppn.0
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

    fn find_pte(&self, vpn: VirtPageNum) -> Option<&'static mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        for (i, &idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.as_pte_array()[idx];
            if i == 2 {
                return if pte.is_valid() { Some(pte) } else { None };
            }
            if !pte.is_valid() {
                return None;
            }
            ppn = pte.ppn();
        }
        unreachable!()
    }

    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).expect("map: no memory");
        assert!(!pte.is_valid(), "vpn {:?} already mapped", vpn);
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }

    /// 重新映射（允许已存在）
    pub fn remap(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).expect("remap: no memory");
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }

    pub fn unmap(&mut self, vpn: VirtPageNum) {
        if let Some(pte) = self.find_pte(vpn) {
            *pte = PageTableEntry::empty();
        }
    }

    pub fn set_flags(&mut self, vpn: VirtPageNum, flags: PTEFlags) -> bool {
        if let Some(pte) = self.find_pte(vpn) {
            pte.set_flags(flags | PTEFlags::V);
            true
        } else {
            false
        }
    }

    pub fn get_flags(&self, vpn: VirtPageNum) -> Option<PTEFlags> {
        self.find_pte(vpn).map(|pte| pte.flags())
    }

    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        self.find_pte(va.floor()).map(|pte| {
            let pa: PhysAddr = pte.ppn().into();
            PhysAddr(pa.0 + va.page_offset())
        })
    }
}
