//! 物理页帧分配器：简单空闲页栈

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

static FREE_PAGES: spin::Mutex<Vec<usize>> = spin::Mutex::new(Vec::new());
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static FREE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub const PAGE_SIZE: usize = 4096;

/// 初始化：把 [start, end) 区间内的所有 4K 页压入空闲栈
pub fn init(start: usize, end: usize) {
    let start = (start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let end = end & !(PAGE_SIZE - 1);
    let mut v = FREE_PAGES.lock();
    let mut p = start;
    while p < end {
        v.push(p);
        p += PAGE_SIZE;
    }
    FREE_COUNT.store(v.len(), Ordering::Relaxed);
    drop(v);
}

/// 分配一个物理页，返回物理地址
pub fn alloc_page() -> Option<usize> {
    let page = FREE_PAGES.lock().pop()?;
    ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    // 清零
    unsafe {
        core::ptr::write_bytes(page as *mut u8, 0, PAGE_SIZE);
    }
    Some(page)
}

/// 释放一个物理页
pub fn free_page(paddr: usize) {
    debug_assert!(paddr & (PAGE_SIZE - 1) == 0);
    FREE_PAGES.lock().push(paddr);
    FREE_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn stats() -> (usize, usize) {
    (ALLOC_COUNT.load(Ordering::Relaxed), FREE_COUNT.load(Ordering::Relaxed))
}

/// spin 锁（极简实现，单核安全）
pub mod spin {
    use core::cell::UnsafeCell;
    use core::ops::{Deref, DerefMut};
    use core::sync::atomic::{AtomicBool, Ordering};

    pub struct Mutex<T> {
        locked: AtomicBool,
        data: UnsafeCell<T>,
    }

    impl<T> Mutex<T> {
        pub const fn new(data: T) -> Self {
            Mutex {
                locked: AtomicBool::new(false),
                data: UnsafeCell::new(data),
            }
        }
        pub fn lock(&self) -> MutexGuard<'_, T> {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
            MutexGuard { lock: self }
        }
        pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
            if self
                .locked
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                Some(MutexGuard { lock: self })
            } else {
                None
            }
        }
    }

    unsafe impl<T: Send> Send for Mutex<T> {}
    unsafe impl<T: Send> Sync for Mutex<T> {}

    pub struct MutexGuard<'a, T> {
        lock: &'a Mutex<T>,
    }

    impl<T> Deref for MutexGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            unsafe { &*self.lock.data.get() }
        }
    }
    impl<T> DerefMut for MutexGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            unsafe { &mut *self.lock.data.get() }
        }
    }
    impl<T> Drop for MutexGuard<'_, T> {
        fn drop(&mut self) {
            self.lock.locked.store(false, Ordering::Release);
        }
    }
}
