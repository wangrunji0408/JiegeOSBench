//! 物理页帧分配器（bump + 回收栈）

use crate::config::{MEMORY_END, PAGE_SIZE};
use crate::mm::addr::{PhysAddr, PhysPageNum};
use crate::sync::UPIntrFreeCell;
use alloc::vec::Vec;
use lazy_static::lazy_static;

lazy_static! {
    static ref FRAME_ALLOCATOR: UPIntrFreeCell<StackFrameAllocator> =
        unsafe { UPIntrFreeCell::new(StackFrameAllocator::new()) };
}

pub struct StackFrameAllocator {
    current: usize, // 下一个可分配的页号
    end: usize,     // 结束页号
    recycled: Vec<usize>,
}

impl StackFrameAllocator {
    pub const fn new() -> Self {
        Self {
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }
    pub fn init(&mut self, start: PhysPageNum, end: PhysPageNum) {
        self.current = start.0;
        self.end = end.0;
    }
    pub fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.recycled.pop() {
            Some(PhysPageNum(ppn))
        } else if self.current < self.end {
            let ppn = self.current;
            self.current += 1;
            Some(PhysPageNum(ppn))
        } else {
            None
        }
    }
    pub fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;
        if ppn >= self.current || self.recycled.contains(&ppn) {
            panic!("frame {:#x} double free or invalid", ppn);
        }
        self.recycled.push(ppn);
    }
}

pub fn init_frame_allocator() {
    extern "C" {
        fn ekernel();
    }
    let start = PhysAddr(ekernel as usize).ceil();
    let end = PhysAddr(MEMORY_END).floor();
    FRAME_ALLOCATOR.lock().init(start, end);
    println!(
        "frame allocator: {:#x} .. {:#x} ({} MiB)",
        start.0 * PAGE_SIZE,
        end.0 * PAGE_SIZE,
        (end.0 - start.0) * PAGE_SIZE / 1024 / 1024
    );
}

/// RAII 帧句柄，Drop 时自动释放
pub struct FrameTracker {
    pub ppn: PhysPageNum,
}

impl FrameTracker {
    pub fn new(ppn: PhysPageNum) -> Self {
        // 清零
        ppn.as_bytes().fill(0);
        Self { ppn }
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        FRAME_ALLOCATOR.lock().dealloc(self.ppn);
    }
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR.lock().alloc().map(FrameTracker::new)
}
