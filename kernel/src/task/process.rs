//! Placeholder for the process/address-space layer (filled in next).
use crate::mm::page_table::PageTable;
use crate::trap::TrapFrame;

pub struct Process {
    pub pid: usize,
    pub ppid: usize,
    pub pgid: usize,
    pub pt: PageTable,
    pub exit_code: i32,
    pub exited: bool,
}

impl Process {
    pub fn new(pid: usize, pt: PageTable) -> Self {
        Self {
            pid,
            ppid: 0,
            pgid: pid,
            pt,
            exit_code: 0,
            exited: false,
        }
    }
}

pub fn handle_page_fault(_tf: &mut TrapFrame, _va: usize, _scause: usize) -> bool {
    false
}

pub fn on_thread_exit(_i: usize) {}
