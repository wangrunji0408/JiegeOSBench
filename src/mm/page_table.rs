//! Sv39 三级页表实现。

use core::ptr::{read_volatile, write_volatile};
use crate::mm::address::*;
use crate::mm::frame::FRAME_ALLOCATOR;
use crate::mm::MEMORY_TOP;

pub const PAGE_SIZE: usize = 4096;

// PTE 标志位
pub const PTE_V: u64 = 1 << 0;
pub const PTE_R: u64 = 1 << 1;
pub const PTE_W: u64 = 1 << 2;
pub const PTE_X: u64 = 1 << 3;
pub const PTE_U: u64 = 1 << 4;
pub const PTE_G: u64 = 1 << 5;
pub const PTE_A: u64 = 1 << 6;
pub const PTE_D: u64 = 1 << 7;

/// Sv39 三级索引
#[inline]
fn vpn2(va: usize) -> usize {
    (va >> 30) & 0x1FF
}
#[inline]
fn vpn1(va: usize) -> usize {
    (va >> 21) & 0x1FF
}
#[inline]
fn vpn0(va: usize) -> usize {
    (va >> 12) & 0x1FF
}

/// PPN 字段在 PTE 中的位 [10:53]
#[inline]
fn pte_from_ppn_flags(ppn: usize, flags: u64) -> u64 {
    ((ppn as u64) << 10) | flags
}
#[inline]
fn pte_ppn(pte: u64) -> usize {
    ((pte >> 10) & 0x3FFFFFFFFFF) as usize
}
#[inline]
fn pte_flags(pte: u64) -> u64 {
    pte & 0x3FF
}

/// 物理地址 -> 可写指针（当前内核身份映射，VA==PA）
#[inline]
fn pa_to_ptr<T>(pa: usize) -> *mut T {
    pa as *mut T
}

/// 一个页表（占用一个 4KB 物理帧，512 个 PTE）
pub struct PageTable {
    pub root_pa: usize,
}

impl PageTable {
    /// 新建空页表（分配根帧并清零）
    pub fn new() -> Option<Self> {
        let root_pa = FRAME_ALLOCATOR.alloc_zeroed()?;
        Some(Self { root_pa })
    }

    /// 用已有根帧构造
    pub fn from_root(root_pa: usize) -> Self {
        Self { root_pa }
    }

    /// 读取某一级表项
    unsafe fn read_pte(table_pa: usize, idx: usize) -> u64 {
        read_volatile((table_pa as *const u64).add(idx))
    }
    unsafe fn write_pte(table_pa: usize, idx: usize, pte: u64) {
        write_volatile((table_pa as *mut u64).add(idx), pte);
    }

    /// 取得/创建下一级页表，返回其物理地址。
    /// level: 2=根, 1=中间, 0=叶子
    unsafe fn walk_create(&self, va: usize, level: usize, flags_leaf: u64) -> usize {
        // 根级索引随 level 变化
        let indices = [vpn2(va), vpn1(va), vpn0(va)];
        let mut table_pa = self.root_pa;
        // 从 level=2 走到目标级
        for l in (0..=2).rev() {
            let idx = indices[l];
            let pte = Self::read_pte(table_pa, idx);
            if l == level {
                // 命中目标级：返回当前表地址（调用方在此写入叶子或大页项）
                return table_pa;
            }
            // 还需向下走
            if pte & PTE_V == 0 {
                // 分配下一级表
                let child_pa = FRAME_ALLOCATOR.alloc_zeroed().expect("OOM in walk_create");
                let child_pte = pte_from_ppn_flags(pa_to_pfn(child_pa), PTE_V);
                Self::write_pte(table_pa, idx, child_pte);
                table_pa = child_pa;
            } else if pte & (PTE_R | PTE_W | PTE_X) != 0 {
                // 已是大页项
                return table_pa;
            } else {
                table_pa = pfn_to_pa(pte_ppn(pte));
            }
        }
        table_pa
    }

    /// 映射一个 4KB 页：va -> pa，给定权限标志
    pub fn map_page(&self, va: usize, pa: usize, flags: u64) {
        assert!(is_page_aligned(va) && is_page_aligned(pa));
        unsafe {
            // 走到 level 0
            let mut table_pa = self.root_pa;
            for l in (1..=2).rev() {
                let idx = [0, 0, vpn2(va), vpn1(va)][l]; // vpn2 at level2, vpn1 at level1
                let idx = if l == 2 { vpn2(va) } else { vpn1(va) };
                let pte = Self::read_pte(table_pa, idx);
                if pte & PTE_V == 0 {
                    let child_pa = FRAME_ALLOCATOR.alloc_zeroed().expect("OOM map_page");
                    let child_pte = pte_from_ppn_flags(pa_to_pfn(child_pa), PTE_V);
                    Self::write_pte(table_pa, idx, child_pte);
                    table_pa = child_pa;
                } else if pte & (PTE_R | PTE_W | PTE_X) != 0 {
                    panic!("map_page: huge page exists where 4K expected");
                } else {
                    table_pa = pfn_to_pa(pte_ppn(pte));
                }
            }
            // level 0
            let idx = vpn0(va);
            let pte = pte_from_ppn_flags(pa_to_pfn(pa), flags | PTE_V | PTE_A | PTE_D);
            Self::write_pte(table_pa, idx, pte);
        }
    }

    /// 映射一段 [va, va+size) 身份映射（va==pa），按 4KB
    pub fn identity_map_range(&self, start: usize, size: usize, flags: u64) {
        let end = page_up(start + size);
        let mut va = page_down(start);
        while va < end {
            self.map_page(va, va, flags);
            va += PAGE_SIZE;
        }
    }

    /// 映射 2MB 大页（leaf at level 1）
    pub fn map_huge(&self, va: usize, pa: usize, flags: u64) {
        assert!(va % HUGE_PAGE_SIZE == 0 && pa % HUGE_PAGE_SIZE == 0);
        unsafe {
            let idx2 = vpn2(va);
            let pte2 = Self::read_pte(self.root_pa, idx2);
            let table1_pa = if pte2 & PTE_V == 0 {
                let child = FRAME_ALLOCATOR.alloc_zeroed().expect("OOM map_huge");
                let child_pte = pte_from_ppn_flags(pa_to_pfn(child), PTE_V);
                Self::write_pte(self.root_pa, idx2, child_pte);
                child
            } else {
                pfn_to_pa(pte_ppn(pte2))
            };
            let idx1 = vpn1(va);
            let pte = pte_from_ppn_flags(pa_to_pfn(pa) >> 9, flags | PTE_V | PTE_A | PTE_D);
            // 注意：大页 PPN 存的是 PA>>12 但只取高位；PTE 的 PPN 字段含 PA[32:12]，
            // 低 9 位（页内 2MB 偏移）由 VA offset 提供，因此直接用 pa_to_pfn(pa) 即可
            let pte = pte_from_ppn_flags(pa_to_pfn(pa), flags | PTE_V | PTE_A | PTE_D);
            Self::write_pte(table1_pa, idx1, pte);
        }
    }

    /// 用 2MB 大页身份映射一段区域
    pub fn identity_map_huge_range(&self, start: usize, size: usize, flags: u64) {
        let end = (start + size + HUGE_PAGE_SIZE - 1) & !(HUGE_PAGE_SIZE - 1);
        let mut va = start & !(HUGE_PAGE_SIZE - 1);
        while va < end {
            self.map_huge(va, va, flags);
            va += HUGE_PAGE_SIZE;
        }
    }

    /// 查找 va 对应的叶子 PTE（用于调试）
    pub fn translate(&self, va: usize) -> Option<(usize, u64)> {
        unsafe {
            let mut table_pa = self.root_pa;
            for l in (0..=2).rev() {
                let idx = if l == 2 { vpn2(va) } else if l == 1 { vpn1(va) } else { vpn0(va) };
                let pte = Self::read_pte(table_pa, idx);
                if pte & PTE_V == 0 {
                    return None;
                }
                if pte & (PTE_R | PTE_W | PTE_X) != 0 {
                    // 叶子
                    let off_bits = if l == 0 { 12 } else if l == 1 { 21 } else { 30 };
                    let pa = (pte_ppn(pte) << 12) | (va & ((1 << off_bits) - 1));
                    return Some((pa, pte));
                }
                table_pa = pfn_to_pa(pte_ppn(pte));
            }
            None
        }
    }

    /// satp 值（Sv39, asid=0）
    pub fn satp(&self) -> usize {
        (8usize << 60) | (self.root_pa >> 12)
    }
}

/// 设置 satp 并刷新 TLB
pub unsafe fn set_satp(satp: usize) {
    core::arch::asm!("csrw satp, {}", in(reg) satp);
    core::arch::asm!("sfence.vma zero, zero");
}

/// 全局内核页表
static mut KERNEL_PT: Option<PageTable> = None;

pub fn kernel_pt() -> &'static PageTable {
    unsafe { KERNEL_PT.as_ref().unwrap() }
}

/// 构建内核地址空间：身份映射全部 RAM + MMIO，然后切换 satp。
pub fn init_kernel() {
    let pt = PageTable::new().expect("failed to alloc kernel page table");

    // 内核 RWX 标志（S-mode 可访问，无 U）
    let ktext = PTE_R | PTE_W | PTE_X | PTE_G;
    let krw = PTE_R | PTE_W | PTE_G;
    let krwx = PTE_R | PTE_W | PTE_X | PTE_G;

    // 用 2MB 大页身份映射整个物理内存 [0x80000000, 0x88000000)
    pt.identity_map_huge_range(0x8000_0000, MEMORY_TOP - 0x8000_0000, krwx);

    // 身份映射 MMIO 区域（UART 等）：0x10000000 一段
    pt.identity_map_huge_range(0x1000_0000, HUGE_PAGE_SIZE, krw);
    // PLIC @ 0x0c000000, 4MB
    pt.identity_map_huge_range(0x0c00_0000, HUGE_PAGE_SIZE * 2, krw);

    // 切换到新页表
    unsafe {
        set_satp(pt.satp());
    }

    unsafe {
        KERNEL_PT = Some(pt);
    }

    crate::println!("[mm] kernel page table active (Sv39, identity-mapped)");
}
