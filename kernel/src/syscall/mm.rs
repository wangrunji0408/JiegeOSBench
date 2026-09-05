//! Memory management system calls.
use crate::abi::*;
use crate::config::PAGE_SIZE;
use crate::mm::addrspace::{page_up, Prot};
use crate::mm::uaccess::current_mm;
use crate::task::current;

fn prot_from(prot: u32) -> Prot {
    let mut p = Prot::empty();
    if prot & PROT_READ != 0 {
        p |= Prot::R;
    }
    if prot & PROT_WRITE != 0 {
        p |= Prot::W;
    }
    if prot & PROT_EXEC != 0 {
        p |= Prot::X;
    }
    p
}

pub fn sys_brk(addr: usize) -> SysResult {
    let mm = current_mm();
    let mut a = mm.lock();
    if addr == 0 {
        return Ok(a.brk);
    }
    Ok(a.set_brk(addr))
}

pub fn sys_mmap(addr: usize, len: usize, prot: u32, flags: u32, fd: i32, off: u64) -> SysResult {
    if len == 0 {
        return Err(EINVAL);
    }
    if addr % PAGE_SIZE != 0 && flags & MAP_FIXED != 0 {
        return Err(EINVAL);
    }
    if off % PAGE_SIZE as u64 != 0 {
        return Err(EINVAL);
    }
    let shared = flags & MAP_SHARED != 0;
    let file = if flags & MAP_ANONYMOUS != 0 {
        None
    } else {
        let f = super::fs::get_file(fd)?;
        if !f.ops.seekable() {
            return Err(ENODEV);
        }
        if !f.readable() {
            return Err(EACCES);
        }
        Some((f, off))
    };
    let fixed = flags & MAP_FIXED != 0;
    if flags & MAP_FIXED_NOREPLACE != 0 {
        let mm = current_mm();
        let a = mm.lock();
        if !a.is_free(addr, addr + page_up(len)) {
            return Err(EEXIST);
        }
        drop(a);
        return sys_mmap(addr, len, prot, (flags & !MAP_FIXED_NOREPLACE) | MAP_FIXED, fd, off);
    }
    let mm = current_mm();
    let mut a = mm.lock();
    // Shared file mappings are treated as private (no page cache sharing);
    // shared anonymous mappings really are shared across fork.
    let shared = shared && file.is_none();
    let start = a.mmap(addr, len, prot_from(prot), shared, fixed, file)?;
    Ok(start)
}

pub fn sys_munmap(addr: usize, len: usize) -> SysResult {
    if addr % PAGE_SIZE != 0 || len == 0 {
        return Err(EINVAL);
    }
    let mm = current_mm();
    mm.lock().munmap(addr, len);
    Ok(0)
}

pub fn sys_mprotect(addr: usize, len: usize, prot: u32) -> SysResult {
    if addr % PAGE_SIZE != 0 {
        return Err(EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    let mm = current_mm();
    mm.lock().mprotect(addr, len, prot_from(prot))?;
    Ok(0)
}

pub fn sys_madvise(_addr: usize, _len: usize, _advice: i32) -> SysResult {
    Ok(0)
}

pub fn sys_mremap(old_addr: usize, old_len: usize, new_len: usize, flags: u32, new_addr: usize) -> SysResult {
    const MREMAP_MAYMOVE: u32 = 1;
    const MREMAP_FIXED: u32 = 2;
    if old_addr % PAGE_SIZE != 0 {
        return Err(EINVAL);
    }
    let old_len = page_up(old_len);
    let new_len = page_up(new_len);
    let mm = current_mm();
    let mut a = mm.lock();
    let vma = a.find_vma(old_addr).ok_or(EFAULT)?.clone();
    if vma.end < old_addr + old_len {
        return Err(EFAULT);
    }
    if new_len <= old_len {
        if new_len < old_len {
            a.munmap(old_addr + new_len, old_len - new_len);
        }
        return Ok(old_addr);
    }
    // Try to grow in place.
    if flags & MREMAP_FIXED == 0 && vma.end == old_addr + old_len && a.extend_vma(vma.start, old_addr + new_len) {
        return Ok(old_addr);
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return Err(ENOMEM);
    }
    // Move: allocate new region, copy contents, unmap old.
    let dst = if flags & MREMAP_FIXED != 0 { new_addr } else { a.find_free(new_len, 0).ok_or(ENOMEM)? };
    let prot = vma.prot;
    let shared = vma.shared;
    a.mmap(dst, new_len, prot, shared, true, None)?;
    drop(a);
    let task = current();
    let mmr = task.mm();
    let mut buf = alloc::vec![0u8; PAGE_SIZE];
    let mut off = 0;
    while off < old_len {
        if crate::mm::uaccess::copy_from_user_mm(&mmr, &mut buf, old_addr + off).is_ok() {
            crate::mm::uaccess::copy_to_user_mm(&mmr, dst + off, &buf)?;
        }
        off += PAGE_SIZE;
    }
    mmr.lock().munmap(old_addr, old_len);
    Ok(dst)
}
