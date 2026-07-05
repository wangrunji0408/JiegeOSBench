//! 内核堆分配器：自旋锁 + 空闲链表，每个分配块带头部记录大小以便释放。

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::mm::{HEAP_START, HEAP_SIZE, PAGE_SIZE};
use crate::mm::frame::FRAME_ALLOCATOR;

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

struct FreeList {
    head: *mut FreeBlock,
}

// FreeList 持有裸指针，跨线程访问由 Spinlock 保护
unsafe impl Send for FreeList {}
unsafe impl Sync for FreeList {}

impl FreeList {
    const fn new() -> Self {
        Self { head: null_mut() }
    }

    /// 初始化：把 [start, start+size) 作为单个大空闲块
    pub unsafe fn init(&mut self, start: usize, size: usize) {
        let block = start as *mut FreeBlock;
        (*block).size = size;
        (*block).next = null_mut();
        self.head = block;
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(16);
        let align = layout.align();
        let size = (size + 15) & !15; // 16 字节对齐大小

        let mut prev: *mut FreeBlock = null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            let cur_size = unsafe { (*cur).size };
            if cur_size >= size {
                // 找到合适块
                if cur_size >= size + 32 {
                    // 分裂
                    let rem = unsafe { (cur as *mut u8).add(size) } as *mut FreeBlock;
                    unsafe {
                        (*rem).size = cur_size - size;
                        (*rem).next = (*cur).next;
                    }
                    unsafe { (*cur).size = size };
                    if prev.is_null() {
                        self.head = rem;
                    } else {
                        unsafe { (*prev).next = rem };
                    }
                } else {
                    // 整块分配
                    let next = unsafe { (*cur).next };
                    if prev.is_null() {
                        self.head = next;
                    } else {
                        unsafe { (*prev).next = next };
                    }
                }
                return cur as *mut u8;
            }
            prev = cur;
            cur = unsafe { (*cur).next };
        }
        null_mut()
    }

    /// 释放：把 [ptr, ptr+size) 当作空闲块插入并按地址排序合并
    pub unsafe fn free(&mut self, ptr: *mut u8, size: usize) {
        let block = ptr as *mut FreeBlock;
        (*block).size = size;
        // 按地址升序插入
        let mut prev: *mut FreeBlock = null_mut();
        let mut cur = self.head;
        while !cur.is_null() && (cur as usize) < (block as usize) {
            prev = cur;
            cur = (*cur).next;
        }
        (*block).next = cur;
        if prev.is_null() {
            self.head = block;
        } else {
            (*prev).next = block;
        }
        // 与后继合并
        if !cur.is_null() && (block as usize) + (*block).size == cur as usize {
            (*block).size += (*cur).size;
            (*block).next = (*cur).next;
        }
        // 与前驱合并
        if !prev.is_null() && (prev as usize) + (*prev).size == block as usize {
            (*prev).size += (*block).size;
            (*prev).next = (*block).next;
        }
    }
}

struct Spinlock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    const fn new(t: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(t),
        }
    }
    fn lock(&self) -> &mut T {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        unsafe { &mut *self.data.get() }
    }
    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct HeapAlloc {
    inner: Spinlock<FreeList>,
}

impl HeapAlloc {
    pub const fn new() -> Self {
        Self {
            inner: Spinlock::new(FreeList::new()),
        }
    }

    pub unsafe fn init(&self) {
        // 在帧分配器中预留堆区域
        let frames = HEAP_SIZE / PAGE_SIZE;
        for i in 0..frames {
            FRAME_ALLOCATOR.mark_used_pa(HEAP_START + i * PAGE_SIZE);
        }
        self.inner.lock().init(HEAP_START, HEAP_SIZE);
        // lock 返回 &mut，drop 时不会自动解锁（我们没实现 Drop），手动解锁：
        // 这里利用 lock 后未显式解锁会一直持锁——因此用一个临时作用域包装。
        // 为简洁，重写为下面的辅助函数。
        unreachable!("init should use init_locked");
    }
}

// 上面的 init 会死锁（持锁不释放）。改为独立初始化函数。
pub fn init_heap() {
    let frames = HEAP_SIZE / PAGE_SIZE;
    for i in 0..frames {
        FRAME_ALLOCATOR.mark_used_pa(HEAP_START + i * PAGE_SIZE);
    }
    unsafe {
        let fl = ALLOCATOR.inner.lock();
        fl.init(HEAP_START, HEAP_SIZE);
        // 释放锁
        ALLOCATOR.inner.unlock();
    }
}

#[global_allocator]
static ALLOCATOR: HeapAlloc = HeapAlloc::new();

unsafe impl GlobalAlloc for HeapAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 头部 16 字节
        let header = 16usize;
        let align = layout.align().max(16);
        let total = (layout.size() + header + align - 1) & !(align - 1);
        let total = total.max(32);

        let fl = self.inner.lock();
        let ptr = fl.alloc(Layout::from_size_align(total, 16).unwrap());
        // 注意：fl 是 &mut，作用域结束前手动解锁
        self.inner.unlock();
        if ptr.is_null() {
            return null_mut();
        }
        // 在头部存总大小
        *(ptr as *mut usize) = total;
        *((ptr as *mut usize).add(1)) = 0xC0DE;
        ptr.add(header)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let header = 16usize;
        let base = ptr.sub(header);
        let total = *(base as *const usize);
        if *((base as *const usize).add(1)) != 0xC0DE {
            return; // 不是我们分配的
        }
        let fl = self.inner.lock();
        fl.free(base as *mut u8, total);
        self.inner.unlock();
    }
}
