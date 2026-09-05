//! Locking primitives for a single-core, non-preemptive (in kernel) design.
//!
//! The kernel runs with interrupts disabled except in the idle loop, so a lock
//! can never be contended by another CPU or by an interrupt handler while the
//! holder is running. Contention therefore always indicates a bug (re-entrancy
//! or blocking while holding a lock), which we report loudly instead of spinning.
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        SpinLock { locked: AtomicBool::new(false), data: UnsafeCell::new(data) }
    }

    #[track_caller]
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        if self.locked.swap(true, Ordering::Acquire) {
            panic!("SpinLock: re-entrant lock detected");
        }
        SpinLockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        if self.locked.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(SpinLockGuard { lock: self })
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// A cell holding kernel-global state that is initialised once during boot and
/// mutated only while interrupts are disabled. Access is unchecked.
pub struct Global<T> {
    data: UnsafeCell<Option<T>>,
}

unsafe impl<T> Sync for Global<T> {}

impl<T> Global<T> {
    pub const fn new() -> Self {
        Global { data: UnsafeCell::new(None) }
    }
    pub fn init(&self, v: T) {
        unsafe { *self.data.get() = Some(v) }
    }
    #[allow(clippy::mut_from_ref)]
    pub fn get(&self) -> &mut T {
        unsafe { (*self.data.get()).as_mut().expect("Global not initialised") }
    }
    pub fn is_init(&self) -> bool {
        unsafe { (*self.data.get()).is_some() }
    }
}
