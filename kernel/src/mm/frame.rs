use alloc::vec::Vec;
use spin::Mutex;
use crate::config::*;

/// 物理页帧
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysFrame(usize); // 页帧号

impl PhysFrame {
    pub fn from_ppn(ppn: usize) -> Self {
        Self(ppn)
    }

    pub fn ppn(&self) -> usize {
        self.0
    }

    pub fn addr(&self) -> usize {
        self.0 << PAGE_SIZE_BITS
    }

    pub fn as_mut_ptr(&self) -> *mut u8 {
        crate::utils::phys_to_virt(self.addr()) as *mut u8
    }

    pub fn zero(&self) {
        unsafe {
            core::ptr::write_bytes(self.as_mut_ptr(), 0, PAGE_SIZE);
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                crate::utils::phys_to_virt(self.addr()) as *const u8,
                PAGE_SIZE,
            )
        }
    }

    pub fn as_mut_slice(&self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.as_mut_ptr(),
                PAGE_SIZE,
            )
        }
    }
}

/// 带有自动释放的页帧
pub struct FrameTracker(pub PhysFrame);

impl FrameTracker {
    pub fn new(frame: PhysFrame) -> Self {
        // Zero the frame. Since we always switch to KERNEL_SPACE before syscall handling
        // (via trap.S from_user path), and kernel initialization runs under KERNEL_SPACE,
        // we can safely zero frames at all times.
        frame.zero();
        Self(frame)
    }

    pub fn new_zeroed(frame: PhysFrame) -> Self {
        frame.zero();
        Self(frame)
    }

    pub fn ppn(&self) -> usize {
        self.0.ppn()
    }

    pub fn addr(&self) -> usize {
        self.0.addr()
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        dealloc_frame(self.0);
    }
}

struct FrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<PhysFrame>,
}

impl FrameAllocator {
    fn new() -> Self {
        Self {
            current: 0,
            end: 0,
            recycled: Vec::new(),
        }
    }

    fn init(&mut self, start: usize, end: usize) {
        self.current = start;
        self.end = end;
    }

    fn alloc(&mut self) -> Option<PhysFrame> {
        if let Some(frame) = self.recycled.pop() {
            Some(frame)
        } else if self.current < self.end {
            let frame = PhysFrame::from_ppn(self.current);
            self.current += 1;
            Some(frame)
        } else {
            None
        }
    }

    fn dealloc(&mut self, frame: PhysFrame) {
        // 简单检查，防止双重释放
        debug_assert!(frame.ppn() < self.current);
        debug_assert!(!self.recycled.contains(&frame));
        self.recycled.push(frame);
    }
}

static FRAME_ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator {
    current: 0,
    end: 0,
    recycled: Vec::new(),
});

pub fn init_frame_allocator() {
    extern "C" {
        fn ekernel();
    }
    let start_pa = crate::utils::virt_to_phys(ekernel as usize);
    let start_ppn = (start_pa + PAGE_SIZE - 1) / PAGE_SIZE;
    let end_ppn = MEMORY_END / PAGE_SIZE;

    println!("[mm] Frame allocator: ppn {} - {}", start_ppn, end_ppn);
    FRAME_ALLOCATOR.lock().init(start_ppn, end_ppn);
}

pub fn alloc_frame() -> Option<FrameTracker> {
    FRAME_ALLOCATOR.lock().alloc().map(FrameTracker::new)
}

pub fn dealloc_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.lock().dealloc(frame);
}
