//! 单处理器自旋锁（关中断保护）

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// 单核环境下使用的锁：通过自旋 + 关 SIE 保证互斥。
/// 内核态默认关中断运行，这里主要防止锁内触发中断重入。
pub struct UPIntrFreeCell<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T> Sync for UPIntrFreeCell<T> {}
unsafe impl<T> Send for UPIntrFreeCell<T> {}

pub struct UPIntrGuard<'a, T> {
    cell: &'a UPIntrFreeCell<T>,
}

impl<T> UPIntrFreeCell<T> {
    /// # Safety
    /// 只能在单核、且内核态关中断的环境下使用。
    pub unsafe fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> UPIntrGuard<'_, T> {
        // 关 SIE，防止持锁期间被中断打断后重入
        let _sie = riscv_sie_read();
        riscv_sie_clear();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        UPIntrGuard { cell: self }
    }

    /// 获取内部数据的裸指针（单核、关中断环境下安全使用）
    pub fn as_ptr(&self) -> *mut T {
        self.data.get()
    }
}

impl<'a, T> Deref for UPIntrGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.cell.data.get() }
    }
}

impl<'a, T> DerefMut for UPIntrGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.cell.data.get() }
    }
}

impl<'a, T> Drop for UPIntrGuard<'a, T> {
    fn drop(&mut self) {
        self.cell.locked.store(false, Ordering::Release);
        // 内核态统一保持 SIE 关闭（用户态由 sret 恢复 SPIE）
        riscv_sie_clear();
    }
}

fn riscv_sie_read() -> usize {
    let x: usize;
    unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) x) };
    x & (1 << 1)
}

fn riscv_sie_clear() {
    unsafe { core::arch::asm!("csrci sstatus, 2") };
}
