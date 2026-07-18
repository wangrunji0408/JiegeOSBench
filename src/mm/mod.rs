mod addr;
mod frame;
mod heap;
mod page_table;
mod space;

pub use addr::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
pub use frame::{frame_alloc, FrameTracker};
pub use page_table::{PTEFlags, PageTable};
pub use space::{AddressSpace, MapArea, MapPerm, KERNEL_SPACE};

pub fn init_heap() {
    heap::init_heap();
}

pub fn init_frame_allocator() {
    frame::init_frame_allocator();
}

/// 供内核读写用户空间：拷贝数据到用户 VA
pub fn copy_out(space: &AddressSpace, mut va: usize, data: &[u8]) -> Result<(), ()> {
    let mut offset = 0;
    while offset < data.len() {
        let page_va = va & !(crate::config::PAGE_SIZE - 1);
        let page_off = va - page_va;
        let len = core::cmp::min(crate::config::PAGE_SIZE - page_off, data.len() - offset);
        let pa = space.translate(va).ok_or(())?;
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(offset), pa as *mut u8, len);
        }
        offset += len;
        va = page_va + crate::config::PAGE_SIZE;
    }
    Ok(())
}

/// 从用户 VA 拷贝数据到内核
pub fn copy_in(space: &AddressSpace, mut va: usize, buf: &mut [u8]) -> Result<(), ()> {
    let mut offset = 0;
    while offset < buf.len() {
        let page_va = va & !(crate::config::PAGE_SIZE - 1);
        let page_off = va - page_va;
        let len = core::cmp::min(crate::config::PAGE_SIZE - page_off, buf.len() - offset);
        let pa = space.translate(va).ok_or(())?;
        unsafe {
            core::ptr::copy_nonoverlapping(pa as *const u8, buf.as_mut_ptr().add(offset), len);
        }
        offset += len;
        va = page_va + crate::config::PAGE_SIZE;
    }
    Ok(())
}

/// 从用户空间读取 C 字符串
pub fn copy_in_str(space: &AddressSpace, mut va: usize) -> Result<alloc::string::String, ()> {
    let mut s = alloc::vec::Vec::new();
    loop {
        let pa = space.translate(va).ok_or(())?;
        let page_off = va & (crate::config::PAGE_SIZE - 1);
        let mut end = pa;
        let page_end = pa - page_off + crate::config::PAGE_SIZE;
        while end < page_end {
            let c = unsafe { *(end as *const u8) };
            if c == 0 {
                return Ok(alloc::string::String::from_utf8_lossy(&s).into_owned());
            }
            s.push(c);
            end += 1;
        }
        va = va - page_off + crate::config::PAGE_SIZE;
        if s.len() > 4096 {
            return Err(());
        }
    }
}
