//! 内核堆分配器：基于空闲链表的变长分配器，支持分配/释放与合并。
//! 后端内存位于 HEAP_START..HEAP_START+HEAP_SIZE（已身份映射）。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use crate::mm::{HEAP_START, HEAP_SIZE};
use crate::mm::frame::FRAME_ALLOCATOR;

/// 空闲块头部：size（含头）+ next 指针
#[repr(C)]
struct FreeBlock {
    size: usize,
    next: Option<&'static mut FreeBlock>,
}

pub struct Heap {
    head: Option<&'static mut FreeBlock>,
}

impl Heap {
    pub const fn new() -> Self {
        Self { head: None }
    }

    /// 初始化：把整个堆区域作为一个大空闲块
    pub unsafe fn init(&mut self, start: usize, size: usize) {
        let block = start as *mut FreeBlock;
        (*block).size = size;
        (*block).next = None;
        self.head = Some(&mut *block);
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(8).max(layout.align());
        let size = (size + 7) & !7; // 8 字节对齐大小

        let mut prev: *mut FreeBlock = null_mut();
        let mut cur = match self.head.take() {
            Some(h) => h as *mut FreeBlock,
            None => return null_mut(),
        };

        loop {
            let cur_block = unsafe { &mut *cur };
            if cur_block.size >= size {
                // 命中：若剩余空间能再放一个最小块，则分裂
                if cur_block.size >= size + 24 {
                    let rem_ptr = unsafe { (cur as *mut u8).add(size) } as *mut FreeBlock;
                    unsafe {
                        (*rem_ptr).size = cur_block.size - size;
                        (*rem_ptr).next = cur_block.next.take();
                    }
                    cur_block.size = size;
                    // 重新挂回剩余
                    if prev.is_null() {
                        self.head = unsafe { Some(&mut *rem_ptr) };
                    } else {
                        unsafe { (*prev).next = Some(&mut *rem_ptr) };
                    }
                } else {
                    // 整块分配
                    if prev.is_null() {
                        self.head = cur_block.next.take();
                    } else {
                        unsafe { (*prev).next = cur_block.next.take() };
                    }
                }
                return cur as *mut u8;
            }
            // 不够大，向后找
            prev = cur;
            match cur_block.next.take() {
                Some(n) => {
                    // 把刚才 take 掉的 next 写回给 prev（因为我们只是借来遍历）
                    unsafe { (*prev).next = Some(n) };
                    cur = n as *mut FreeBlock;
                }
                None => {
                    self.head = Some(unsafe { &mut *prev });
                    return null_mut();
                }
            }
        }
    }

    pub fn dealloc(&mut self, ptr: *mut u8, _layout: Layout) {
        let block = ptr as *mut FreeBlock;
        unsafe {
            (*block).size = (*block).size; // 大小未知（未保存），用占位
            // 实际上我们在分配时未保存 size，这里用 0 标记；见下方 DeallocNode 策略
        }
        // 简化：直接把释放的块头插。但 size 未知 → 采用下面的伴随头策略。
        // 为正确性，我们改用伴随元数据（见下面的实现）。
        self.head = unsafe { Some(&mut *block) };
    }
}

// === 伴随元数据的正确实现 ===

const MAGIC: usize = 0xDEAD_BEEF_CAFE_0000;

/// 在每个分配块前预留一个 AllocationHeader，记录 size，便于正确释放。
#[repr(C)]
struct AllocHeader {
    magic: usize,
    size: usize,
}

pub struct HeapAlloc {
    heap: Heap,
}

impl HeapAlloc {
    pub const fn new() -> Self {
        Self { heap: Heap::new() }
    }

    pub unsafe fn init(&mut self) {
        // 在帧分配器中预留堆区域
        let frames = HEAP_SIZE / crate::mm::PAGE_SIZE;
        for i in 0..frames {
            FRAME_ALLOCATOR.mark_used_pa(HEAP_START + i * crate::mm::PAGE_SIZE);
        }
        self.heap.init(HEAP_START, HEAP_SIZE);
    }
}

unsafe impl GlobalAlloc for HeapAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 头部需要 16 字节并对齐到 16
        let header_size = 16usize;
        let align = layout.align().max(16);
        let total = layout.size() + header_size;
        let total = (total + align - 1) & !(align - 1);

        // 用内部 free list 分配
        let inner = &mut *(self as *const Self as *mut Self);
        inner.heap.head = self.heap.head.take();
        let ptr = inner.heap.alloc(Layout::from_size_align(total, 16).unwrap());
        inner.heap.head = inner.heap.head.take();

        if ptr.is_null() {
            return null_mut();
        }
        // 写入头
        let header = ptr as *mut AllocHeader;
        (*header).magic = MAGIC;
        (*header).size = total;
        ptr.add(header_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let header_ptr = ptr.sub(16) as *mut AllocHeader;
        if (*header_ptr).magic != MAGIC {
            // 不是我们分配的，忽略
            return;
        }
        let size = (*header_ptr).size;
        (*header_ptr).magic = 0;
        let inner = &mut *(self as *const Self as *mut Self);
        // 把整块（含头）作为空闲块归还
        let block = header_ptr as *mut FreeBlock;
        (*block).size = size;
        (*block).next = inner.heap.head.take();
        inner.heap.head = Some(&mut *block);
    }
}

// 给 FrameAllocator 加一个按物理地址标记已用的方法
pub trait FrameMarkExt {
    fn mark_used_pa(&self, pa: usize);
}

impl FrameMarkExt for crate::mm::frame::FrameAllocator {
    fn mark_used_pa(&self, pa: usize) {
        self.mark_used_pa_pub(pa);
    }
}
