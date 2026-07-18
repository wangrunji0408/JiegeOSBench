//! Address spaces: a `MemorySet` is a page table plus the list of mapped
//! areas needed to reconstruct/tear it down.

use super::address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::frame_allocator::{frame_alloc, FrameTracker};
use super::page_table::{PTEFlags, PageTable};
use crate::config::{MEMORY_END, MMIO, PAGE_SIZE};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::arch::asm;

bitflags! {
    #[derive(Copy, Clone, Debug)]
    pub struct MapPermission: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

impl From<MapPermission> for PTEFlags {
    fn from(p: MapPermission) -> Self {
        PTEFlags::from_bits_truncate(p.bits())
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MapType {
    Identical,
    Framed,
}

pub struct MapArea {
    pub vpn_start: VirtPageNum,
    pub vpn_end: VirtPageNum,
    pub data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    pub map_type: MapType,
    pub perm: MapPermission,
}

impl MapArea {
    pub fn new(start_va: VirtAddr, end_va: VirtAddr, map_type: MapType, perm: MapPermission) -> Self {
        Self {
            vpn_start: start_va.floor(),
            vpn_end: end_va.ceil(),
            data_frames: BTreeMap::new(),
            map_type,
            perm,
        }
    }

    pub fn from_existing(vpn_start: VirtPageNum, vpn_end: VirtPageNum, perm: MapPermission) -> Self {
        Self {
            vpn_start,
            vpn_end,
            data_frames: BTreeMap::new(),
            map_type: MapType::Framed,
            perm,
        }
    }

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn = match self.map_type {
            MapType::Identical => PhysPageNum(vpn.0),
            MapType::Framed => {
                let frame = frame_alloc().expect("out of physical memory");
                let ppn = frame.ppn;
                self.data_frames.insert(vpn, frame);
                ppn
            }
        };
        page_table.map(vpn, ppn, PTEFlags::from(self.perm));
    }

    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        if self.map_type == MapType::Framed {
            self.data_frames.remove(&vpn);
        }
        page_table.unmap(vpn);
    }

    pub fn map(&mut self, page_table: &mut PageTable) {
        let mut vpn = self.vpn_start;
        while vpn.0 < self.vpn_end.0 {
            self.map_one(page_table, vpn);
            vpn.0 += 1;
        }
    }

    pub fn unmap(&mut self, page_table: &mut PageTable) {
        let mut vpn = self.vpn_start;
        while vpn.0 < self.vpn_end.0 {
            self.unmap_one(page_table, vpn);
            vpn.0 += 1;
        }
    }

    /// Copy raw bytes into this area starting at its first page, as used
    /// when loading ELF segment contents. `data` may be shorter than the
    /// area (the remainder is left zeroed by the fresh frame allocation).
    pub fn copy_data(&mut self, page_table: &PageTable, data: &[u8]) {
        let mut start = 0;
        let mut vpn = self.vpn_start;
        loop {
            let src = &data[start..data.len().min(start + PAGE_SIZE)];
            let ppn = page_table.translate(vpn).unwrap().ppn();
            let dst = &mut ppn.as_bytes()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= data.len() {
                break;
            }
            vpn.0 += 1;
        }
    }
}

pub struct MemorySet {
    pub page_table: PageTable,
    pub areas: Vec<MapArea>,
    /// Bump allocator for anonymous/file-backed `mmap` regions with no
    /// caller-specified address; never reclaimed on `munmap`, which is a
    /// fine trade for this workload's modest mmap traffic.
    pub mmap_top: usize,
}

impl MemorySet {
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
            mmap_top: crate::config::MMAP_BASE,
        }
    }

    pub fn token(&self) -> usize {
        self.page_table.token()
    }

    pub fn push(&mut self, mut area: MapArea, data: Option<&[u8]>) {
        area.map(&mut self.page_table);
        if let Some(data) = data {
            area.copy_data(&self.page_table, data);
        }
        self.areas.push(area);
    }

    pub fn insert_framed_area(&mut self, start_va: VirtAddr, end_va: VirtAddr, perm: MapPermission) {
        self.push(MapArea::new(start_va, end_va, MapType::Framed, perm), None);
    }

    pub fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some(idx) = self.areas.iter().position(|a| a.vpn_start == start_vpn) {
            let mut area = self.areas.remove(idx);
            area.unmap(&mut self.page_table);
        }
    }

    fn map_identical(&mut self, start: usize, end: usize, perm: MapPermission) {
        self.push(
            MapArea::new(VirtAddr(start), VirtAddr(end), MapType::Identical, perm),
            None,
        );
    }

    /// Kernel address space: identity-maps kernel code/data plus all free
    /// physical RAM plus MMIO regions, so kernel pointers keep working
    /// unchanged after paging is enabled.
    pub fn new_kernel() -> Self {
        unsafe extern "C" {
            fn stext();
            fn etext();
            fn srodata();
            fn erodata();
            fn sdata();
            fn edata();
            fn sbss();
            fn ebss();
            fn ekernel();
        }
        let mut memory_set = Self::new_bare();
        memory_set.map_identical(
            stext as usize,
            etext as usize,
            MapPermission::R | MapPermission::X,
        );
        memory_set.map_identical(srodata as usize, erodata as usize, MapPermission::R);
        memory_set.map_identical(
            sdata as usize,
            edata as usize,
            MapPermission::R | MapPermission::W,
        );
        memory_set.map_identical(
            sbss as usize,
            ebss as usize,
            MapPermission::R | MapPermission::W,
        );
        memory_set.map_identical(
            ekernel as usize,
            MEMORY_END,
            MapPermission::R | MapPermission::W,
        );
        for &(base, len) in MMIO {
            memory_set.map_identical(base, base + len, MapPermission::R | MapPermission::W);
        }
        memory_set
    }

    /// Activate this address space by writing `satp` and flushing the TLB.
    pub fn activate(&self) {
        let satp = self.token();
        unsafe {
            asm!("csrw satp, {}", "sfence.vma", in(reg) satp);
        }
    }
}
