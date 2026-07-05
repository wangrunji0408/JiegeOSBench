//! The (single) user task: address space bookkeeping, fd table, trap frame.
use crate::fs::FdTable;
use crate::mm::paging::PageTable;
use crate::mm::{frame, paging, PAGE_SIZE};
use crate::trap::TrapFrame;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};

pub const ELF_BASE: usize = 0x4000_0000;
pub const INTERP_BASE: usize = 0x4800_0000;
pub const MMAP_BASE: usize = 0x5000_0000;
pub const STACK_TOP: usize = 0x7800_0000;
pub const STACK_SIZE: usize = 4 * 1024 * 1024;

pub struct Task {
    pub tf: TrapFrame,
    pub pt: PageTable,
    pub brk_start: usize,
    pub brk: usize,
    pub mmap_next: usize,
    pub fds: FdTable,
    pub cwd: String,
    pub exit_code: Option<i32>,
    /// All mapped user pages (va -> pa), so munmap can free frames.
    pub pages: BTreeMap<usize, usize>,
    pub tid_address: usize,
    pub sigactions: BTreeMap<usize, [usize; 4]>,
    pub sigmask: u64,
}

impl Task {
    pub fn new() -> Self {
        Task {
            tf: TrapFrame::new(),
            pt: PageTable::new(),
            brk_start: 0,
            brk: 0,
            mmap_next: MMAP_BASE,
            fds: FdTable::new(),
            cwd: "/".to_string(),
            exit_code: None,
            pages: BTreeMap::new(),
            tid_address: 0,
            sigactions: BTreeMap::new(),
            sigmask: 0,
        }
    }

    /// Map a fresh zeroed frame at `va` with PTE flags; returns pa.
    pub fn map_page(&mut self, va: usize, flags: usize) -> usize {
        debug_assert!(va % PAGE_SIZE == 0);
        if let Some(&pa) = self.pages.get(&va) {
            // already mapped: just update flags
            self.pt.protect(va, flags);
            return pa;
        }
        let pa = frame::alloc();
        self.pt.map(va, pa, flags);
        self.pages.insert(va, pa);
        pa
    }

    /// Ensure range [va, va+len) is mapped with flags.
    pub fn map_range(&mut self, va: usize, len: usize, flags: usize) {
        let start = crate::mm::page_down(va);
        let end = crate::mm::page_up(va + len);
        let mut p = start;
        while p < end {
            self.map_page(p, flags);
            p += PAGE_SIZE;
        }
    }

    pub fn unmap_range(&mut self, va: usize, len: usize) {
        let start = crate::mm::page_down(va);
        let end = crate::mm::page_up(va + len);
        let mut p = start;
        while p < end {
            if let Some(pa) = self.pages.remove(&p) {
                self.pt.unmap(p);
                frame::free(pa);
            }
            p += PAGE_SIZE;
        }
        paging::flush_tlb();
    }

    pub fn mmap_alloc(&mut self, len: usize) -> usize {
        let va = self.mmap_next;
        self.mmap_next += crate::mm::page_up(len) + PAGE_SIZE; // guard gap
        va
    }

    /// Copy bytes into the user address space at va (must be mapped).
    pub fn write_user(&self, va: usize, data: &[u8]) {
        // Kernel runs with SUM=1 and the user page table active.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len()) };
    }
}

pub static mut TASK: Option<Task> = None;

/// Single-threaded kernel: this is safe by construction (no interrupts).
#[allow(static_mut_refs)]
pub fn current() -> &'static mut Task {
    unsafe { TASK.as_mut().expect("no task") }
}

#[allow(static_mut_refs)]
pub fn set_current(t: Task) {
    unsafe { TASK = Some(t) };
}

pub fn user_ranges_dump() -> BTreeSet<usize> {
    current().pages.keys().cloned().collect()
}
