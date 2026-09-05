//! Physical page frames, allocated from the kernel heap (identity mapped).
use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::config::PAGE_SIZE;

pub static FRAMES_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

/// An owned 4 KiB physical frame. Freed on drop.
pub struct Frame {
    pa: usize,
}

impl Frame {
    pub fn alloc_zeroed() -> Option<Frame> {
        let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
        let p = unsafe { alloc_zeroed(layout) };
        if p.is_null() {
            return None;
        }
        FRAMES_ALLOCATED.fetch_add(1, Ordering::Relaxed);
        Some(Frame { pa: p as usize })
    }

    pub fn alloc() -> Frame {
        Self::alloc_zeroed().expect("out of memory: frame")
    }

    #[inline]
    pub fn pa(&self) -> usize {
        self.pa
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.pa as *const u8, PAGE_SIZE) }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.pa as *mut u8, PAGE_SIZE) }
    }

    pub fn copy_from(&self, other: &Frame) {
        self.as_mut_slice().copy_from_slice(other.as_slice());
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
        unsafe { dealloc(self.pa as *mut u8, layout) };
        FRAMES_ALLOCATED.fetch_sub(1, Ordering::Relaxed);
    }
}

pub type SharedFrame = Arc<Frame>;
