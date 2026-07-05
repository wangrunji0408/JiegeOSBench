//! 地址类型与常量。

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

/// Sv39 页表相关
pub const PTE_COUNT: usize = 512; // 每个页表 512 项
pub const VPN_BITS: usize = 9;
pub const VA_BITS: usize = 39;

/// 4MB 巨页相关（Sv39 用 2MB 大页，9 位偏移 + 12 = 21）
pub const HUGE_PAGE_SIZE: usize = 1 << 21; // 2MB
pub const HUGE_PAGE_SHIFT: usize = 21;

#[inline]
pub fn page_down(addr: usize) -> usize {
    addr & !(PAGE_SIZE - 1)
}

#[inline]
pub fn page_up(addr: usize) -> usize {
    (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

#[inline]
pub fn is_page_aligned(addr: usize) -> bool {
    addr & (PAGE_SIZE - 1) == 0
}

/// 物理地址 -> 页帧号
#[inline]
pub fn pa_to_pfn(pa: usize) -> usize {
    pa >> PAGE_SHIFT
}

#[inline]
pub fn pfn_to_pa(pfn: usize) -> usize {
    pfn << PAGE_SHIFT
}
