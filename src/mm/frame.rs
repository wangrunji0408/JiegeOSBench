//! 物理帧分配器：位图方式管理 [kernel_end, MEMORY_TOP) 的 4KB 物理页帧。

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::mm::{MEMORY_TOP, PAGE_SIZE};
use crate::mm::address::{pa_to_pfn, pfn_to_pa};

/// 128MB / 4KB = 32768 帧，位图 4096 字节
const MAX_FRAMES: usize = (0x8800_0000usize - 0x8000_0000) / PAGE_SIZE;
#[repr(align(4096))]
struct BitmapBuf([u8; MAX_FRAMES / 8]);
static mut BITMAP: BitmapBuf = BitmapBuf([0u8; MAX_FRAMES / 8]);

#[inline]
fn bitmap_byte(off: usize) -> *mut u8 {
    unsafe { BITMAP.0.as_mut_ptr().add(off / 8) }
}

pub struct FrameAllocator {
    base_pfn: AtomicUsize,
    num_frames: AtomicUsize,
    next_hint: AtomicUsize,
    inited: AtomicBool,
}

pub static FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::new();

impl FrameAllocator {
    pub const fn new() -> Self {
        Self {
            base_pfn: AtomicUsize::new(0),
            num_frames: AtomicUsize::new(0),
            next_hint: AtomicUsize::new(0),
            inited: AtomicBool::new(false),
        }
    }

    pub fn init(&self, kernel_end_pa: usize) {
        let base_pa = 0x8000_0000usize;
        self.base_pfn.store(pa_to_pfn(base_pa), Ordering::SeqCst);

        let total = (MEMORY_TOP - base_pa) / PAGE_SIZE;
        self.num_frames.store(total, Ordering::SeqCst);

        // 全部标记已用
        for i in 0..total {
            self.mark_used(i);
        }
        // 释放 [kernel_end, MEMORY_TOP)
        let ke = crate::mm::address::page_up(kernel_end_pa);
        let start_off = pa_to_pfn(ke) - pa_to_pfn(base_pa);
        let end_off = pa_to_pfn(MEMORY_TOP) - pa_to_pfn(base_pa);
        for i in start_off..end_off {
            self.mark_free(i);
        }
        self.next_hint.store(start_off, Ordering::SeqCst);
        self.inited.store(true, Ordering::SeqCst);

        crate::println!(
            "[mm] frame allocator: usable {:#x}..{:#x} ({} frames free)",
            ke, MEMORY_TOP, end_off - start_off
        );
    }

    fn bit(&self, off: usize) -> bool {
        unsafe {
            let byte = core::ptr::read_volatile(bitmap_byte(off));
            (byte >> (off % 8)) & 1 == 1
        }
    }
    fn set_bit(&self, off: usize, used: bool) {
        unsafe {
            let p = bitmap_byte(off);
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

    /// 按物理地址标记某帧已用（用于预留堆等区域）
    pub fn mark_used_pa(&self, pa: usize) {
        let off = pa_to_pfn(pa).saturating_sub(self.base_pfn.load(Ordering::SeqCst));
        if off < self.num_frames.load(Ordering::SeqCst) {
            self.mark_used(off);
        }
    }

    pub fn alloc(&self) -> Option<usize> {
        if !self.inited.load(Ordering::SeqCst) {
            return None;
        }
        let total = self.num_frames.load(Ordering::SeqCst);
        let base = self.base_pfn.load(Ordering::SeqCst);
        let mut i = self.next_hint.load(Ordering::SeqCst);
        for _ in 0..total {
            if i >= total {
                i = 0;
            }
            if !self.bit(i) {
                self.mark_used(i);
                self.next_hint.store(i + 1, Ordering::SeqCst);
                return Some(pfn_to_pa(i + base));
            }
            i += 1;
        }
        None
    }

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

    pub fn dealloc(&self, pa: usize) {
        let off = pa_to_pfn(pa).saturating_sub(self.base_pfn.load(Ordering::SeqCst));
        if off < self.num_frames.load(Ordering::SeqCst) {
            self.mark_free(off);
        }
    }

    /// 分配 n 个连续物理页，返回首帧物理地址
    pub fn alloc_contig(&self, n: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        if n == 1 {
            return self.alloc();
        }
        let total = self.num_frames.load(Ordering::SeqCst);
        let base = self.base_pfn.load(Ordering::SeqCst);
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
                    return Some(pfn_to_pa(start + base));
                }
            } else {
                run = 0;
            }
            i += 1;
        }
        None
    }

    /// 分配 n 个连续页并清零
    pub fn alloc_contig_zeroed(&self, n: usize) -> Option<usize> {
        let pa = self.alloc_contig(n)?;
        unsafe {
            let p = pa as *mut u8;
            for i in 0..(n * PAGE_SIZE) {
                core::ptr::write_volatile(p.add(i), 0);
            }
        }
        Some(pa)
    }
}

pub fn init(kernel_end_pa: usize) {
    FRAME_ALLOCATOR.init(kernel_end_pa);
}
