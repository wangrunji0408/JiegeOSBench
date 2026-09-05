use crate::config::*;
use crate::file::FdTable;
use crate::page_table::*;
use crate::trap::{TrapContext, __restore};
use crate::{frame, memory};
use core::cell::UnsafeCell;
use core::mem::size_of;

// Dedicated kernel stack used while handling traps for the (single) user task.
#[repr(align(16))]
struct KStack([u8; 1024 * 1024]);
static mut KSTACK: KStack = KStack([0; 1024 * 1024]);

fn kstack_top() -> usize {
    unsafe { (&raw const KSTACK.0 as usize) + 1024 * 1024 }
}

pub struct Task {
    pub pt: PageTable,
    pub brk: usize,
    pub mmap_top: usize,
    pub fds: FdTable,
    pub tid_address: usize,
    pub cx: *mut TrapContext,
}

struct TaskCell(UnsafeCell<Option<Task>>);
unsafe impl Sync for TaskCell {}
static TASK: TaskCell = TaskCell(UnsafeCell::new(None));

pub fn install(task: Task) {
    unsafe {
        *TASK.0.get() = Some(task);
    }
}

pub fn current() -> &'static mut Task {
    unsafe { (*TASK.0.get()).as_mut().unwrap() }
}

impl Task {
    pub fn new(pt: PageTable) -> Self {
        Task {
            pt,
            brk: 0,
            mmap_top: MMAP_BASE,
            fds: FdTable::new(),
            tid_address: 0,
            cx: core::ptr::null_mut(),
        }
    }

    /// Map `size` bytes at user va with the given RWX permission bits (U added).
    pub fn map_user(&self, va: usize, size: usize, perm: usize) {
        let start = va & !(PAGE_SIZE - 1);
        let end = (va + size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut a = start;
        while a < end {
            if self.pt.translate(a).is_none() {
                let pa = frame::alloc();
                self.pt.map(a, pa, perm | PTE_U);
            }
            a += PAGE_SIZE;
        }
    }

    pub fn ensure_mapped(&self, va: usize, size: usize, perm: usize) {
        self.map_user(va, size, perm);
    }

    /// Enter user mode for the first time.
    pub fn enter_user(&mut self, entry: usize, user_sp: usize) -> ! {
        let cx_addr = kstack_top() - size_of::<TrapContext>();
        let cx = cx_addr as *mut TrapContext;
        unsafe {
            (*cx).x = [0; 32];
            (*cx).x[2] = user_sp;
            // sstatus: SPP=0 (return to U), SPIE=1, SUM=1 (kernel can access user).
            let mut sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            sstatus &= !(1 << 8); // SPP = 0
            sstatus |= 1 << 5; // SPIE = 1
            sstatus |= 1 << 18; // SUM = 1
            (*cx).sstatus = sstatus;
            (*cx).sepc = entry;
            self.cx = cx;
            memory::activate(self.pt.satp());
            __restore(cx);
        }
    }
}
