//! PID allocation.

use alloc::vec::Vec;
use spin::Mutex;

struct PidAllocator {
    current: usize,
    recycled: Vec<usize>,
}

impl PidAllocator {
    const fn new() -> Self {
        Self {
            current: 0,
            recycled: Vec::new(),
        }
    }

    fn alloc(&mut self) -> usize {
        if let Some(pid) = self.recycled.pop() {
            pid
        } else {
            self.current += 1;
            self.current - 1
        }
    }

    fn dealloc(&mut self, pid: usize) {
        debug_assert!(pid < self.current);
        debug_assert!(!self.recycled.iter().any(|&p| p == pid));
        self.recycled.push(pid);
    }
}

static PID_ALLOCATOR: Mutex<PidAllocator> = Mutex::new(PidAllocator::new());

pub struct PidHandle(pub usize);

impl Drop for PidHandle {
    fn drop(&mut self) {
        PID_ALLOCATOR.lock().dealloc(self.0);
    }
}

pub fn pid_alloc() -> PidHandle {
    PidHandle(PID_ALLOCATOR.lock().alloc())
}
