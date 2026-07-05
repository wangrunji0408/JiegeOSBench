//! 物理帧分配器：位图方式管理 [kernel_end, MEMORY_TOP) 的 4KB 物理页帧。

use core::sync::atomic::{AtomicBool, Ordering};
use crate::mm::{MEMORY_TOP, PAGE_SIZE};
use crate::mm::address::{pa_to_pfn, pfn_to_pa};

/// 静态位图。128MB / 4KB = 32768 帧，位图 4096 字节。
const MAX_FRAMES: usize = (0x8800_0000usize - 0x8000_0000) / PAGE_SIZE;
static mut BITMAP: [u8; MAX_FRAMES / 8] = [0; MAX_FRAMES / 8];

pub struct FrameAllocator {
    base_pfn: usize,
    num_frames: usize,
    next_hint: usize, // 从这里开始线性扫描
    inited: AtomicBool,
}

pub static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::new();

impl FrameAllocator {
    const fn new() -> Self {
        Self {
            base_pfn: 0,
            num_frames: 0,
            next_hint: 0,
            inited: AtomicBool::new(false),
        }
    }

    /// 初始化：把 [kernel_end_pa, MEMORY_TOP) 标记为可用，其余标记为已用。
    pub fn init(&self, kernel_end_pa: usize) {
        let base_pa = 0x8000_0000usize;
        self.set_base(base_pa);

        // 默认全部置为已用（位图按字节 0xFF = 8 帧全用）
        let total = (MEMORY_TOP - base_pa) / PAGE_SIZE;
        for i in 0..total {
            self.mark_used(i);
        }
        // 释放 [kernel_end, MEMORY_TOP) 范围
        let ke = crate::mm::address::page_up(kernel_end_pa);
        let start_pfn_off = pa_to_pfn(ke) - pa_to_pfn(base_pa);
        let end_pfn_off = pa_to_pfn(MEMORY_TOP) - pa_to_pfn(base_pa);
        for i in start_pfn_off..end_pfn_off {
            self.mark_free(i);
        }
        self.set_total(end_pfn_off);
        self.next_hint_set(start_pfn_off);
        self.inited.store(true, Ordering::SeqCst);

        crate::println!(
            "[mm] frame allocator: {:#x}..{:#x}, usable from {:#x} ({} frames free)",
            base_pa, MEMORY_TOP, ke, end_pfn_off - start_pfn_off
        );
    }

    fn set_base(&self, base_pa: usize) {
        // base_pfn 存到静态；用 unsafe 写入，因为 self 是只读 static
        unsafe {
            let p = &self.base_pfn as *const usize as *mut usize;
            core::ptr::write_volatile(p, pa_to_pfn(base_pa));
        }
    }
    fn set_total(&self, n: usize) {
        unsafe {
            let p = &self.num_frames as *const usize as *mut usize;
            core::ptr::write_volatile(p, n);
        }
    }
    fn next_hint_set(&self, h: usize) {
        unsafe {
            let p = &self.next_hint as *const usize as *mut usize;
            core::ptr::write_volatile(p, h);
        }
    }

    fn bit(&self, off: usize) -> bool {
        unsafe {
            let byte = BITMAP[off / 8];
            (byte >> (off % 8)) & 1 == 1
        }
    }
    fn set_bit(&self, off: usize, used: bool) {
        unsafe {
            let p = &mut BITMAP[off / 8] as *mut u8;
            let mut b = core::ptr::read_volatile(p);
            if used {
                b |= 1 << (off % 8);
            } else {
                b &= !(1 << (off % 8));
            }
            core::ptr::write_volatile(p, b);
        }
    }
    fn mark_used(&self, off: usize) {
        self.set_bit(off, true);
    }
    fn mark_free(&self, off: usize) {
        self.set_bit(off, false);
    }

    /// 分配一个 4KB 物理页帧，返回其物理地址；无可用则 None。
    pub fn alloc(&self) -> Option<usize> {
        if !self.inited.load(Ordering::SeqCst) {
            return None;
        }
        let total = self.num_frames;
        let mut i = self.next_hint;
        for _ in 0..total {
            if i >= total {
                i = 0;
            }
            if !self.bit(i) {
                self.mark_used(i);
                unsafe {
                    let p = &self.next_hint as *const usize as *mut usize;
                    core::ptr::write_volatile(p, i + 1);
                }
                let pa = pfn_to_pa(i + self.base_pfn);
                return Some(pa);
            }
            i += 1;
        }
        None
    }

    /// 分配并清零
    pub fn alloc_zeroed(&self) -> Option<usize> {
        let pa = self.alloc()?;
        unsafe {
            let p = pa as *mut u8;
            for i in 0..PAGE_SIZE {
                core::ptr::write_volatile(p.add(i), 0);
            }
        }
        Some(pa)
    }

    /// 释放一个物理页帧
    pub fn dealloc(&self, pa: usize) {
        let off = pa_to_pfn(pa).saturating_sub(self.base_pfn);
        if off < self.num_frames {
            self.mark_free(off);
        }
    }

    /// 分配若干连续物理页（用于页表根等），返回首帧物理地址
    pub fn alloc_contig(&self, n: usize) -> Option<usize> {
        // 简化：连续分配 n 帧
        if n == 0 {
            return None;
        }
        if n == 1 {
            return self.alloc();
        }
        let total = self.num_frames;
        let mut run = 0;
        let mut start = 0;
        let mut i = 0;
        while i < total {
            if !self.bit(i) {
                if run == 0 {
                    start = i;
                }
                run += 1;
                if run == n {
                    for k in start..start + n {
                        self.mark_used(k);
                    }
                    return Some(pfn_to_pa(start + self.base_pfn));
                }
            } else {
                run = 0;
            }
            i += 1;
        }
        None
    }
}

pub fn init(kernel_end_pa: usize) {
    FRAME_ALLOCATOR.init(kernel_end_pa);
}
