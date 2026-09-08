pub mod address;
pub mod frame;
pub mod heap;
pub mod page_table;

pub use address::*;
pub use page_table::{PageTable, PTE_A, PTE_D, PTE_G, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X};

/// Boot-time memory information gathered from the device tree.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryMap {
    pub ram_start: usize,
    pub ram_end: usize,
    pub initrd_start: usize,
    pub initrd_end: usize,
}

static mut MEMORY_MAP: MemoryMap = MemoryMap {
    ram_start: 0,
    ram_end: 0,
    initrd_start: 0,
    initrd_end: 0,
};

pub fn set_memory_map(m: MemoryMap) {
    unsafe {
        MEMORY_MAP = m;
    }
}

pub fn memory_map() -> MemoryMap {
    unsafe { MEMORY_MAP }
}
