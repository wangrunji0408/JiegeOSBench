//! SV39 页表管理
//!
//! 内核运行在恒等映射（VA == PA）之上；每个用户进程页表额外恒等映射
//! 全部物理内存与 MMIO 区域（U=0），用户态无法访问这些映射。

use crate::pmm;
use crate::pmm::spin::Mutex;

pub const PAGE_SIZE: usize = 4096;
pub const VPN_BITS: usize = 9;
pub const ENTRIES: usize = 512;
pub const SATP_MODE_SV39: u64 = 8 << 60;

// PTE 标志位
pub const PTE_V: u64 = 1 << 0;
pub const PTE_R: u64 = 1 << 1;
pub const PTE_W: u64 = 1 << 2;
pub const PTE_X: u64 = 1 << 3;
pub const PTE_U: u64 = 1 << 4;
pub const PTE_A: u64 = 1 << 6;
pub const PTE_D: u64 = 1 << 7;

pub type PageTable = [u64; ENTRIES];

#[inline]
fn pa_to_ppn(pa: usize) -> u64 {
    (pa >> 12) as u64
}

#[inline]
pub fn ppn_to_pa(ppn: u64) -> usize {
    (ppn as usize) << 12
}

/// 分配一个清零页表
pub fn alloc_table() -> Option<usize> {
    let pa = pmm::alloc_page()?;
    // alloc_page 已清零
    Some(pa)
}

/// 三级页表查找/建立。返回叶子 PTE 的指针（不存在则按需分配中间级）。
/// leaf=true 时该函数返回目标叶子位置；写入由调用者负责。
unsafe fn walk_alloc(root: usize, va: usize) -> Option<*mut u64> {
    let mut table = root as *mut PageTable;
    for level in (1..=2).rev() {
        let idx = (va >> (12 + level * 9)) & (ENTRIES - 1);
        let pte = (*table).as_mut_ptr().add(idx);
        let v = pte.read_volatile();
        if v & PTE_V == 0 {
            let pa = alloc_table()?;
            let flags = PTE_V; // 中间节点：无 RWX
            pte.write_volatile(pa_to_ppn(pa) << 10 | flags);
        } else if v & (PTE_R | PTE_W | PTE_X) != 0 {
            // 已经是大页叶子
            return None;
        }
        table = ppn_to_pa(v >> 10) as *mut PageTable;
    }
    let idx = va >> 12 & (ENTRIES - 1);
    Some((*table).as_mut_ptr().add(idx))
}

/// 映射一个 4K 页
pub fn map_4k(root: usize, va: usize, pa: usize, flags: u64) -> bool {
    unsafe {
        match walk_alloc(root, va) {
            Some(pte) => {
                pte.write_volatile((pa_to_ppn(pa) << 10) | flags | PTE_V);
                true
            }
            None => false,
        }
    }
}

/// 映射一个 2MB 大页（va/pa 需 2MB 对齐）
pub fn map_2m(root: usize, va: usize, pa: usize, flags: u64) -> bool {
    debug_assert!(va & 0x1f_ffff == 0 && pa & 0x1f_ffff == 0);
    unsafe {
        // 手动走两级到 L1
        let mut table = root as *mut PageTable;
        for level in [2usize] {
            let idx = (va >> (12 + level * 9)) & (ENTRIES - 1);
            let pte = (*table).as_mut_ptr().add(idx);
            let v = pte.read_volatile();
            if v & PTE_V == 0 {
                let pa_t = alloc_table().expect("oom alloc table");
                pte.write_volatile(pa_to_ppn(pa_t) << 10 | PTE_V);
            }
            table = ppn_to_pa(v >> 10) as *mut PageTable;
            // 注意：刚分配时 v 是旧值，重新读
            let idx2 = (va >> (12 + level * 9)) & (ENTRIES - 1);
            table = ppn_to_pa((*table).as_ptr().add(idx2).read_volatile() >> 10) as *mut PageTable;
        }
        let idx = (va >> (12 + 9)) & (ENTRIES - 1);
        let pte = (*table).as_mut_ptr().add(idx);
        pte.write_volatile((pa_to_ppn(pa) << 10) | flags | PTE_V | PTE_A | PTE_D);
        true
    }
}

/// 查找 va 的叶子 PTE（不建立），返回 (物理地址, flags)；大页也归一化处理
pub fn lookup(root: usize, va: usize) -> Option<(usize, u64)> {
    unsafe {
        let mut table = root as *mut PageTable;
        for level in (1..=2).rev() {
            let idx = (va >> (12 + level * 9)) & (ENTRIES - 1);
            let v = (*table).as_ptr().add(idx).read_volatile();
            if v & PTE_V == 0 {
                return None;
            }
            if v & (PTE_R | PTE_W | PTE_X) != 0 {
                // 大页
                let base = (v >> 10) as usize << 12;
                let mask = (1usize << (12 + level * 9)) - 1;
                return Some((base | (va & mask), v));
            }
            table = ppn_to_pa(v >> 10) as *mut PageTable;
        }
        let idx = va >> 12 & (ENTRIES - 1);
        let v = (*table).as_ptr().add(idx).read_volatile();
        if v & PTE_V == 0 {
            return None;
        }
        Some((ppn_to_pa(v >> 10), v))
    }
}

/// 修改已有 PTE 的权限标志
pub fn remap_flags(root: usize, va: usize, flags: u64) -> bool {
    unsafe {
        if let Some(pte) = walk_existing(root, va) {
            let v = pte.read_volatile();
            pte.write_volatile((v & !0xff) | flags | PTE_V);
            true
        } else {
            false
        }
    }
}

unsafe fn walk_existing(root: usize, va: usize) -> Option<*mut u64> {
    let mut table = root as *mut PageTable;
    for level in (1..=2).rev() {
        let idx = (va >> (12 + level * 9)) & (ENTRIES - 1);
        let v = (*table).as_ptr().add(idx).read_volatile();
        if v & PTE_V == 0 {
            return None;
        }
        if v & (PTE_R | PTE_W | PTE_X) != 0 {
            return Some((*table).as_mut_ptr().add(idx));
        }
        table = ppn_to_pa(v >> 10) as *mut PageTable;
    }
    let idx = va >> 12 & (ENTRIES - 1);
    Some((*table).as_mut_ptr().add(idx))
}

/// 撤销 va 处的映射（叶子）
pub fn unmap(root: usize, va: usize) -> bool {
    unsafe {
        if let Some(pte) = walk_existing(root, va) {
            pte.write_volatile(0);
            true
        } else {
            false
        }
    }
}

#[inline]
pub fn sfence_vma() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero");
    }
}

#[inline]
pub fn read_satp() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("csrr {}, satp", out(reg) v);
    }
    v
}

/// 装载页表基址（root 为物理地址）
#[inline]
pub fn load_satp(root_paddr: usize) {
    let satp = SATP_MODE_SV39 | ((root_paddr >> 12) as u64);
    unsafe {
        core::arch::asm!("csrw satp, {}", in(reg) satp);
        sfence_vma();
    }
}

// ------------------------- 内核映射 -------------------------

/// 全局记录：RAM 范围与 MMIO 范围，用于给每个用户页表复制内核映射
pub static KERNEL_REGIONS: Mutex<(usize, usize, usize, usize)> = Mutex::new((0, 0, 0, 0));

pub fn record_kernel_regions(ram_start: usize, ram_end: usize, mmio_start: usize, mmio_end: usize) {
    *KERNEL_REGIONS.lock() = (ram_start, ram_end, mmio_start, mmio_end);
}

/// 在给定页表中建立内核区恒等映射（大页，无 U 位）
pub fn map_kernel_regions(root: usize) {
    let (ram_start, ram_end, mmio_start, mmio_end) = *KERNEL_REGIONS.lock();
    let two_m = 0x20_0000usize;
    let mut va = ram_start & !(two_m - 1);
    while va < ram_end {
        map_2m(root, va, va, PTE_R | PTE_W | PTE_X | PTE_A | PTE_D);
        va += two_m;
    }
    let mut va = mmio_start & !(two_m - 1);
    while va < mmio_end {
        map_2m(root, va, va, PTE_R | PTE_W | PTE_X | PTE_A | PTE_D);
        va += two_m;
    }
}

/// 初始化内核页表并开启分页（恒等映射）
pub fn init_kernel_paging(ram_start: usize, ram_end: usize, mmio_start: usize, mmio_end: usize) -> usize {
    let root = alloc_table().expect("cannot alloc kernel page table");
    record_kernel_regions(ram_start, ram_end, mmio_start, mmio_end);
    map_kernel_regions(root);
    load_satp(root);
    root
}
