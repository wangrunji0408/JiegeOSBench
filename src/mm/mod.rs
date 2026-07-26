//! Memory management.

pub mod addr;
pub mod frame;
pub mod heap;
pub mod page_table;
pub mod uaccess;
pub mod vma;

pub use addr::*;
pub use vma::{AddrSpace, Backing, Prot, Vma};

extern "C" {
    fn ekernel();
}

pub fn init() {
    heap::init();
    let kernel_end = ekernel as usize;
    frame::init(kernel_end, MEMORY_END);
    let (_, total) = frame::stats();
    crate::info!(
        "physical frames: {} available ({} MiB) from {:#x}",
        total,
        total * PAGE_SIZE / 1024 / 1024,
        page_up(kernel_end)
    );
    vma::init_kernel_page_table();
}
