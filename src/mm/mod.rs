//! Memory management: kernel heap, physical frame allocator, Sv39 page tables.
pub mod frame;
pub mod heap;
pub mod paging;

pub const PAGE_SIZE: usize = 4096;

pub const fn page_down(a: usize) -> usize {
    a & !(PAGE_SIZE - 1)
}
pub const fn page_up(a: usize) -> usize {
    (a + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub fn init() {
    heap::init();
    frame::init();
}
