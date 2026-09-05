pub mod addrspace;
pub mod frame;
pub mod heap;
pub mod page_table;
pub mod uaccess;

use crate::config::*;

extern "C" {
    static __kernel_end: u8;
}

/// Initialise the heap with all RAM not used by the kernel image or the rootfs
/// archive. `rootfs_end` is the end of the cpio archive placed by QEMU.
pub fn init(rootfs_end: usize) {
    let kend = unsafe { &__kernel_end as *const u8 as usize };
    let kend = (kend + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let rend = (rootfs_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    unsafe {
        heap::add_region(kend, ROOTFS_ADDR);
        heap::add_region(rend, RAM_END);
    }
    klog!("heap: {:#x}-{:#x}, {:#x}-{:#x}", kend, ROOTFS_ADDR, rend, RAM_END);
}
