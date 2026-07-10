use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bitflags::bitflags;
use spin::Mutex;
use lazy_static::lazy_static;
use crate::config::*;
use super::frame::{alloc_frame, FrameTracker, PhysFrame};
use super::page_table::{PageTable, PTEFlags};

bitflags! {
    #[derive(Copy, Clone)]
    pub struct MapPerm: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

impl From<MapPerm> for PTEFlags {
    fn from(perm: MapPerm) -> Self {
        PTEFlags::from_bits_truncate(perm.bits())
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum MapType {
    /// 直接映射（内核使用，物理地址 = 虚拟地址 - 偏移）
    Identical,
    /// 分配物理页帧
    Framed,
}

pub struct MapArea {
    pub vpn_range: core::ops::Range<usize>,
    pub frames: BTreeMap<usize, FrameTracker>,
    pub map_type: MapType,
    pub perm: MapPerm,
}

impl MapArea {
    pub fn new(
        va_start: usize,
        va_end: usize,
        map_type: MapType,
        perm: MapPerm,
    ) -> Self {
        let vpn_start = va_start / PAGE_SIZE;
        let vpn_end = (va_end + PAGE_SIZE - 1) / PAGE_SIZE;
        Self {
            vpn_range: vpn_start..vpn_end,
            frames: BTreeMap::new(),
            map_type,
            perm,
        }
    }

    pub fn start_va(&self) -> usize {
        self.vpn_range.start * PAGE_SIZE
    }

    pub fn end_va(&self) -> usize {
        self.vpn_range.end * PAGE_SIZE
    }

    pub fn perm(&self) -> MapPerm {
        self.perm
    }

    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: usize) {
        let ppn = match self.map_type {
            MapType::Identical => {
                let va = vpn * PAGE_SIZE;
                crate::utils::virt_to_phys(va) / PAGE_SIZE
            }
            MapType::Framed => {
                let frame = alloc_frame().expect("out of memory");
                let ppn = frame.ppn();
                self.frames.insert(vpn, frame);
                ppn
            }
        };
        let flags = PTEFlags::from(self.perm);
        page_table.map(vpn, ppn, flags);
    }

    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range.clone() {
            self.map_one(page_table, vpn);
        }
    }

    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range.clone() {
            page_table.unmap(vpn);
            self.frames.remove(&vpn);
        }
    }

    pub fn copy_data(&mut self, page_table: &mut PageTable, data: &[u8]) {
        let mut start = 0;
        for vpn in self.vpn_range.clone() {
            let frame = self.frames.get(&vpn).unwrap();
            let len = (data.len() - start).min(PAGE_SIZE);
            if len == 0 { break; }
            frame.0.as_mut_slice()[..len].copy_from_slice(&data[start..start + len]);
            start += len;
        }
    }
}

pub struct MemorySet {
    pub page_table: PageTable,
    pub areas: Vec<MapArea>,
}

impl MemorySet {
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        self.page_table.token()
    }

    pub fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    pub fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    pub fn push(&mut self, mut area: MapArea, data: Option<&[u8]>) {
        area.map(&mut self.page_table);
        if let Some(data) = data {
            area.copy_data(&mut self.page_table, data);
        }
        self.areas.push(area);
    }

    pub fn insert_framed_area(&mut self, va_start: usize, va_end: usize, perm: MapPerm) {
        self.push(
            MapArea::new(va_start, va_end, MapType::Framed, perm),
            None,
        );
    }

    /// 移除一个虚拟地址范围的映射
    pub fn remove_area_with_start_va(&mut self, va_start: usize) -> bool {
        if let Some(pos) = self.areas.iter().position(|a| a.start_va() == va_start) {
            self.areas[pos].unmap(&mut self.page_table);
            self.areas.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn activate(&self) -> usize {
        let satp = self.page_table.token();
        unsafe {
            riscv::register::satp::write(satp);
            core::arch::asm!("sfence.vma");
        }
        satp
    }

    /// 为内核创建地址空间（直接映射）
    /// 物理地址 = 虚拟地址 (KERNEL_OFFSET = 0)
    pub fn new_kernel() -> Self {
        let mut ms = Self::new_bare();

        // 映射低位设备IO区域 (0x00000000 - 0x10009000)
        ms.push(MapArea::new(
            0x00001000,  // 跳过地址0
            0x10009000,
            MapType::Identical,
            MapPerm::R | MapPerm::W,
        ), None);

        // 映射内核代码和数据（从物理地址0x80200000开始）
        extern "C" {
            fn ekernel();
        }
        let kernel_end = (ekernel as usize + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        ms.push(MapArea::new(
            KERNEL_PHYS_BASE,
            kernel_end,
            MapType::Identical,
            MapPerm::R | MapPerm::W | MapPerm::X,
        ), None);

        // 映射剩余物理内存（用于帧分配器）
        ms.push(MapArea::new(
            kernel_end,
            MEMORY_END,
            MapType::Identical,
            MapPerm::R | MapPerm::W,
        ), None);

        ms
    }

    /// 翻译虚拟地址
    pub fn translate(&self, va: usize) -> Option<usize> {
        self.page_table.translate_va(va)
    }

    /// 从用户空间读取数据
    pub fn copy_from_user(&self, src_va: usize, dst: &mut [u8]) {
        self.page_table.copy_from_user(src_va, dst);
    }

    /// 向用户空间写入数据
    pub fn copy_to_user(&self, dst_va: usize, src: &[u8]) {
        self.page_table.copy_to_user(dst_va, src);
    }

    /// 读取用户空间字符串
    pub fn read_cstr(&self, va: usize) -> alloc::string::String {
        self.page_table.read_cstr(va)
    }

    /// 从另一个内存集克隆（用于fork）
    pub fn clone_for_child(&self) -> Self {
        let mut child = Self::new_bare();
        // 先添加内核映射（与父进程相同）
        crate::task::elf::map_kernel_into_public(&mut child);
        // 克隆所有用户映射区域
        for area in &self.areas {
            // Skip areas that might be in kernel space (VPN2=2 range)
            if area.start_va() >= 0x80000000 { continue; }
            let mut new_area = MapArea::new(
                area.start_va(),
                area.end_va(),
                MapType::Framed,
                area.perm(),
            );
            new_area.map(&mut child.page_table);
            // 复制数据
            for vpn in area.vpn_range.clone() {
                if let Some(src_frame) = area.frames.get(&vpn) {
                    if let Some(dst_frame) = new_area.frames.get(&vpn) {
                        dst_frame.0.as_mut_slice().copy_from_slice(src_frame.0.as_slice());
                    }
                }
            }
            child.areas.push(new_area);
        }
        child
    }
}

lazy_static! {
    pub static ref KERNEL_SPACE: Mutex<MemorySet> = Mutex::new(MemorySet::new_kernel());
}

/// Global kernel SATP token, set after kernel page table is initialized
/// Used to safely access kernel page table frames from any page table context
/// Exported as simple symbol for assembly access
#[no_mangle]
pub static KERNEL_SATP: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

pub fn init_kernel_space() {
    let satp = KERNEL_SPACE.lock().activate();
    KERNEL_SATP.store(satp, core::sync::atomic::Ordering::SeqCst);
    println!("[mm] Kernel address space activated (Sv39)");
}
