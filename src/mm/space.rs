//! 地址空间：内核空间（恒等映射 + trampoline）与用户空间

use crate::config::{MEMORY_END, PAGE_SIZE, TRAMPOLINE};
use crate::mm::addr::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use crate::mm::frame::{frame_alloc, FrameTracker};
use crate::mm::page_table::{PTEFlags, PageTable, PageTableEntry};
use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct MapPerm: u8 {
        const R = 1 << 0;
        const W = 1 << 1;
        const X = 1 << 2;
        const U = 1 << 3;
    }
}

impl MapPerm {
    pub fn to_pte_flags(&self) -> PTEFlags {
        let mut f = PTEFlags::A | PTEFlags::D;
        if self.contains(MapPerm::R) {
            f |= PTEFlags::R;
        }
        if self.contains(MapPerm::W) {
            f |= PTEFlags::W;
        }
        if self.contains(MapPerm::X) {
            f |= PTEFlags::X;
        }
        if self.contains(MapPerm::U) {
            f |= PTEFlags::U;
        }
        f
    }
}

pub struct MapArea {
    pub start: VirtPageNum,
    pub end: VirtPageNum, // 不含
    pub frames: BTreeMap<VirtPageNum, FrameTracker>,
    pub perm: MapPerm,
}

impl MapArea {
    pub fn new(start_va: VirtAddr, end_va: VirtAddr, perm: MapPerm) -> Self {
        Self {
            start: start_va.floor(),
            end: end_va.ceil(),
            frames: BTreeMap::new(),
            perm,
        }
    }
}

pub struct AddressSpace {
    pub page_table: PageTable,
    pub areas: Vec<MapArea>,
}

lazy_static! {
    pub static ref KERNEL_SPACE: UPIntrFreeCell<AddressSpace> =
        unsafe { UPIntrFreeCell::new(AddressSpace::new_kernel()) };
}

extern "C" {
    fn strampoline();
}

impl AddressSpace {
    /// 内核地址空间：恒等映射低 1GiB（MMIO）与 RAM，外加 trampoline
    fn new_kernel() -> Self {
        let mut space = Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        };
        // 1GiB 巨页恒等映射：0x0..0x4000_0000（MMIO 区）
        // 以及 0x8000_0000..0xC000_0000（RAM）
        let root = space.page_table.root_ppn.as_pte_array();
        let gflags = PTEFlags::V | PTEFlags::R | PTEFlags::W | PTEFlags::X | PTEFlags::G
            | PTEFlags::A | PTEFlags::D;
        root[0] = PageTableEntry::new(PhysPageNum(0), gflags);
        root[2] = PageTableEntry::new(PhysPageNum(0x8000_0000 >> 12), gflags);
        // trampoline 映射到所有地址空间共享的高 VA
        space.map_trampoline();
        println!(
            "kernel space: root ppn {:#x}, mem end {:#x}",
            space.page_table.root_ppn.0,
            MEMORY_END
        );
        space
    }

    pub fn kernel_token() -> usize {
        KERNEL_SPACE.lock().page_table.token()
    }

    pub fn activate_kernel() {
        let token = Self::kernel_token();
        unsafe {
            core::arch::asm!("csrw satp, {}", in(reg) token);
            core::arch::asm!("sfence.vma");
        }
    }

    pub fn new_user() -> Self {
        let mut space = Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        };
        space.map_trampoline();
        space
    }

    fn map_trampoline(&mut self) {
        let pa = PhysAddr(strampoline as usize);
        self.page_table.map(
            VirtAddr(TRAMPOLINE).floor(),
            pa.floor(),
            PTEFlags::R | PTEFlags::X | PTEFlags::G | PTEFlags::A,
        );
    }

    pub fn token(&self) -> usize {
        self.page_table.token()
    }

    pub fn translate(&self, va: usize) -> Option<usize> {
        self.page_table.translate_va(VirtAddr(va)).map(|pa| pa.0)
    }

    /// 映射一个区域并按页分配帧，可选拷贝初始数据（数据长度可小于区域）
    pub fn map_area(&mut self, mut area: MapArea, data: Option<&[u8]>) {
        let flags = area.perm.to_pte_flags();
        let mut vpn = area.start;
        while vpn < area.end {
            let frame = frame_alloc().expect("map_area: out of memory");
            if let Some(d) = data {
                let page_start: usize = VirtAddr::from(vpn).0;
                let area_start: usize = VirtAddr::from(area.start).0;
                if page_start >= area_start && page_start - area_start < d.len() {
                    let src_off = page_start - area_start;
                    let len = core::cmp::min(PAGE_SIZE, d.len() - src_off);
                    frame.ppn.as_bytes()[..len].copy_from_slice(&d[src_off..src_off + len]);
                }
            }
            self.page_table.map(vpn, frame.ppn, flags);
            area.frames.insert(vpn, frame);
            vpn.0 += 1;
        }
        self.areas.push(area);
    }

    /// 在指定 VA 映射单页并写入数据（用于内核栈顶页等特殊映射）
    pub fn map_page_at(&mut self, va: VirtAddr, ppn: PhysPageNum, perm: MapPerm) {
        self.page_table.remap(va.floor(), ppn, perm.to_pte_flags());
    }

    /// 解除 [start, end) 的映射并释放帧
    pub fn unmap_range(&mut self, start: VirtAddr, end: VirtAddr) {
        let start_vpn = start.floor();
        let end_vpn = end.ceil();
        let mut i = 0;
        while i < self.areas.len() {
            // 检查是否相交
            if self.areas[i].end <= start_vpn || self.areas[i].start >= end_vpn {
                i += 1;
                continue;
            }
            let area = &mut self.areas[i];
            let lo = core::cmp::max(area.start, start_vpn);
            let hi = core::cmp::min(area.end, end_vpn);
            let mut vpn = lo;
            while vpn < hi {
                if area.frames.remove(&vpn).is_some() {
                    self.page_table.unmap(vpn);
                }
                vpn.0 += 1;
            }
            // 处理区域收缩/分裂
            let area = &mut self.areas[i];
            if lo <= area.start && hi >= area.end {
                // 整个区域被移除
                self.areas.remove(i);
                continue;
            } else if lo <= area.start {
                area.start = hi;
            } else if hi >= area.end {
                area.end = lo;
            } else {
                // 中间挖洞：分裂成两个区域
                let mut right = MapArea {
                    start: hi,
                    end: area.end,
                    frames: BTreeMap::new(),
                    perm: area.perm,
                };
                let mut vpn = hi;
                while vpn < right.end {
                    if let Some(f) = area.frames.remove(&vpn) {
                        right.frames.insert(vpn, f);
                    }
                    vpn.0 += 1;
                }
                area.end = lo;
                self.areas.insert(i + 1, right);
            }
            i += 1;
        }
    }

    /// 修改 [start, end) 内已映射页的属性
    pub fn protect_range(&mut self, start: VirtAddr, end: VirtAddr, perm: MapPerm) -> bool {
        let start_vpn = start.floor();
        let end_vpn = end.ceil();
        let flags = perm.to_pte_flags();
        let mut vpn = start_vpn;
        let mut ok = true;
        while vpn < end_vpn {
            if !self.page_table.set_flags(vpn, flags) {
                ok = false;
            }
            vpn.0 += 1;
        }
        // 更新区域 perm 记录（简化：只更新完全覆盖的区域）
        for area in self.areas.iter_mut() {
            if area.start >= start_vpn && area.end <= end_vpn {
                area.perm = perm;
            }
        }
        ok
    }

    /// 检查 [start, end) 是否全部已映射
    pub fn is_mapped(&self, start: VirtAddr, end: VirtAddr) -> bool {
        let mut vpn = start.floor();
        let end_vpn = end.ceil();
        while vpn < end_vpn {
            if self.page_table.get_flags(vpn).is_none() {
                return false;
            }
            vpn.0 += 1;
        }
        true
    }

    /// 寻找 [start, end) 是否与已有区域重叠
    pub fn range_free(&self, start: VirtPageNum, end: VirtPageNum) -> bool {
        self.areas
            .iter()
            .all(|a| a.end <= start || a.start >= end)
    }

    /// fork：完整复制地址空间（深拷贝所有页）
    pub fn fork_copy(&self) -> Self {
        let mut new_space = Self::new_user();
        for area in &self.areas {
            let mut new_area = MapArea {
                start: area.start,
                end: area.end,
                frames: BTreeMap::new(),
                perm: area.perm,
            };
            let flags = area.perm.to_pte_flags();
            let mut vpn = area.start;
            while vpn < area.end {
                if let Some(frame) = area.frames.get(&vpn) {
                    let new_frame = frame_alloc().expect("fork: out of memory");
                    new_frame.ppn.as_bytes().copy_from_slice(frame.ppn.as_bytes());
                    new_space.page_table.map(vpn, new_frame.ppn, flags);
                    new_area.frames.insert(vpn, new_frame);
                }
                vpn.0 += 1;
            }
            new_space.areas.push(new_area);
        }
        new_space
    }
}
