//! Task control block.

use super::context::TaskContext;
use super::pid::{pid_alloc, PidHandle};
use crate::config::{KERNEL_STACK_SIZE, TRAP_CONTEXT};
use crate::fs::File;
use crate::mm::{kernel_token, MemorySet, PhysPageNum, VirtAddr};
use crate::signal::SignalState;
use crate::trap::{trap_handler, TrapContext};
use alloc::alloc::{alloc, dealloc};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::alloc::Layout;
use spin::{Mutex, MutexGuard};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TaskStatus {
    Ready,
    Running,
    Zombie,
}

struct KernelStack {
    ptr: *mut u8,
    layout: Layout,
}

impl KernelStack {
    fn new() -> Self {
        let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16).unwrap();
        let ptr = unsafe { alloc(layout) };
        assert!(!ptr.is_null(), "out of memory allocating kernel stack");
        Self { ptr, layout }
    }
    fn top(&self) -> usize {
        self.ptr as usize + KERNEL_STACK_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) };
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}

pub struct TaskControlBlock {
    pub pid: PidHandle,
    inner: Mutex<TaskControlBlockInner>,
}

pub struct TaskControlBlockInner {
    kernel_stack: KernelStack,
    pub trap_cx_ppn: PhysPageNum,
    pub base_size: usize,
    pub task_cx: TaskContext,
    pub task_status: TaskStatus,
    pub memory_set: MemorySet,
    pub parent: Option<Weak<TaskControlBlock>>,
    pub children: Vec<Arc<TaskControlBlock>>,
    pub exit_code: i32,
    pub heap_bottom: usize,
    pub program_brk: usize,
    pub fd_table: Vec<Option<Arc<dyn File>>>,
    pub cwd: alloc::string::String,
    pub signals: SignalState,
}

impl TaskControlBlockInner {
    pub fn trap_cx(&self) -> &'static mut TrapContext {
        unsafe { &mut *(self.trap_cx_ppn.as_mut_ptr() as *mut TrapContext) }
    }

    pub fn user_token(&self) -> usize {
        self.memory_set.token()
    }

    fn status(&self) -> TaskStatus {
        self.task_status
    }

    /// Find a free slot in the fd table (extending it if necessary) and
    /// install `file` there.
    pub fn alloc_fd(&mut self, file: Arc<dyn File>) -> usize {
        if let Some(fd) = self.fd_table.iter().position(|f| f.is_none()) {
            self.fd_table[fd] = Some(file);
            fd
        } else {
            self.fd_table.push(Some(file));
            self.fd_table.len() - 1
        }
    }

    pub fn get_fd(&self, fd: usize) -> Option<Arc<dyn File>> {
        self.fd_table.get(fd).and_then(|f| f.clone())
    }
}

impl TaskControlBlock {
    pub fn inner_lock(&self) -> MutexGuard<'_, TaskControlBlockInner> {
        self.inner.lock()
    }

    pub fn pid(&self) -> usize {
        self.pid.0
    }

    /// Build the very first process from an ELF image (no parent).
    pub fn new_initproc(elf_data: &[u8], args: &[alloc::string::String], envs: &[alloc::string::String]) -> Arc<Self> {
        let (memory_set, user_sp, entry_point, heap_bottom) = MemorySet::from_elf(elf_data, args, envs);
        let trap_cx_ppn = memory_set
            .page_table
            .translate(VirtAddr(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new();
        let kernel_stack_top = kernel_stack.top();
        let tcb = Self {
            pid,
            inner: Mutex::new(TaskControlBlockInner {
                kernel_stack,
                trap_cx_ppn,
                base_size: user_sp,
                task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                task_status: TaskStatus::Ready,
                memory_set,
                parent: None,
                children: Vec::new(),
                exit_code: 0,
                heap_bottom,
                program_brk: heap_bottom,
                fd_table: crate::fs::stdio_fd_table(),
                cwd: alloc::string::String::from("/"),
            }),
        };
        let tcb = Arc::new(tcb);
        {
            let mut inner = tcb.inner_lock();
            let trap_cx = inner.trap_cx();
            *trap_cx = TrapContext::app_init_context(
                entry_point,
                user_sp,
                kernel_token(),
                kernel_stack_top,
                trap_handler as usize,
            );
        }
        tcb
    }

    /// Duplicate this task (deep-copies the address space). The new task
    /// is registered as a child but not yet placed on the ready queue --
    /// callers do that themselves.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        let mut parent_inner = self.inner_lock();
        let memory_set = MemorySet::from_existing(&parent_inner.memory_set);
        let trap_cx_ppn = memory_set
            .page_table
            .translate(VirtAddr(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();
        let pid = pid_alloc();
        let kernel_stack = KernelStack::new();
        let kernel_stack_top = kernel_stack.top();
        let child = Arc::new(Self {
            pid,
            inner: Mutex::new(TaskControlBlockInner {
                kernel_stack,
                trap_cx_ppn,
                base_size: parent_inner.base_size,
                task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                task_status: TaskStatus::Ready,
                memory_set,
                parent: Some(Arc::downgrade(self)),
                children: Vec::new(),
                exit_code: 0,
                heap_bottom: parent_inner.heap_bottom,
                program_brk: parent_inner.program_brk,
                fd_table: parent_inner.fd_table.clone(),
                cwd: parent_inner.cwd.clone(),
            }),
        });
        {
            // The raw page copy carried over the parent's kernel_sp; fix it
            // up to the child's own freshly allocated stack. kernel_satp
            // and trap_handler are process-independent, so those survive
            // the copy unchanged.
            let child_inner = child.inner_lock();
            child_inner.trap_cx().kernel_sp = kernel_stack_top;
        }
        parent_inner.children.push(child.clone());
        child
    }

    /// Replace this task's address space with a fresh one loaded from
    /// `elf_data`, as `execve` does. The pid, fd table, and parent/child
    /// links are unchanged.
    pub fn exec(&self, elf_data: &[u8], args: &[alloc::string::String], envs: &[alloc::string::String]) {
        let (memory_set, user_sp, entry_point, heap_bottom) = MemorySet::from_elf(elf_data, args, envs);
        let trap_cx_ppn = memory_set
            .page_table
            .translate(VirtAddr(TRAP_CONTEXT).into())
            .unwrap()
            .ppn();
        let mut inner = self.inner_lock();
        let kernel_stack_top = inner.kernel_stack.top();
        inner.memory_set = memory_set;
        inner.trap_cx_ppn = trap_cx_ppn;
        inner.base_size = user_sp;
        inner.heap_bottom = heap_bottom;
        inner.program_brk = heap_bottom;
        *inner.trap_cx() = TrapContext::app_init_context(
            entry_point,
            user_sp,
            kernel_token(),
            kernel_stack_top,
            trap_handler as *const () as usize,
        );
    }
}
