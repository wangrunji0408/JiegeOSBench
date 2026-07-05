//! Memory syscalls: mmap family and brk.
use super::*;
use crate::fs::FdObj;
use crate::mm::paging::{prot_to_flags, PTE_R, PTE_U, PTE_W};
use crate::mm::{page_up, PAGE_SIZE};
use crate::task::current;
use alloc::vec::Vec;

const MAP_SHARED: usize = 0x01;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_FIXED: usize = 0x10;

pub fn mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: isize, off: usize) -> SysResult {
    if len == 0 {
        return Err(EINVAL);
    }
    let t = current();
    let va = if flags & MAP_FIXED != 0 {
        if addr % PAGE_SIZE != 0 {
            return Err(EINVAL);
        }
        // replace: drop old frames so we get fresh zeroed ones
        t.unmap_range(addr, len);
        addr
    } else {
        t.mmap_alloc(len)
    };
    let pte_flags = if prot == 0 {
        PTE_U | PTE_R // PROT_NONE: keep readable; user won't touch it
    } else {
        prot_to_flags(prot)
    };
    t.map_range(va, len, pte_flags);

    if flags & MAP_ANONYMOUS == 0 {
        // file-backed: copy contents page-by-page via physical addresses,
        // since the mapping may lack write permission (e.g. text segments)
        let e = t.fds.get(fd as usize).ok_or(EBADF)?;
        let FdObj::File { data, .. } = &e.obj else {
            return Err(EBADF);
        };
        let d = data.lock();
        if off < d.len() {
            let n = len.min(d.len() - off);
            let src = &d[off..off + n];
            let mut copied = 0;
            while copied < n {
                let page_va = crate::mm::page_down(va + copied);
                let pa = *t.pages.get(&page_va).unwrap();
                let page_off = (va + copied) - page_va;
                let chunk = (PAGE_SIZE - page_off).min(n - copied);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr().add(copied),
                        (pa + page_off) as *mut u8,
                        chunk,
                    );
                }
                copied += chunk;
            }
        }
    }
    let _ = flags & MAP_SHARED; // shared anon: same behavior in single-process world
    crate::mm::paging::flush_tlb();
    Ok(va)
}

pub fn munmap(addr: usize, len: usize) -> SysResult {
    current().unmap_range(addr, len);
    Ok(0)
}

pub fn mprotect(addr: usize, len: usize, prot: usize) -> SysResult {
    let t = current();
    let flags = if prot == 0 {
        PTE_U | PTE_R
    } else {
        prot_to_flags(prot)
    };
    let mut p = crate::mm::page_down(addr);
    let end = page_up(addr + len);
    while p < end {
        t.pt.protect(p, flags);
        p += PAGE_SIZE;
    }
    crate::mm::paging::flush_tlb();
    Ok(0)
}

pub fn brk(addr: usize) -> SysResult {
    let t = current();
    if addr == 0 {
        return Ok(t.brk);
    }
    if addr < t.brk_start {
        return Ok(t.brk);
    }
    if addr > t.brk {
        t.map_range(t.brk, addr - t.brk, PTE_U | PTE_R | PTE_W);
    }
    t.brk = addr;
    Ok(t.brk)
}

pub fn mremap(old: usize, old_len: usize, new_len: usize, _flags: usize, _new: usize) -> SysResult {
    let t = current();
    if new_len <= old_len {
        // shrink in place
        t.unmap_range(old + page_up(new_len), old_len - page_up(new_len));
        return Ok(old);
    }
    // move: allocate fresh region, copy, free old
    let va = t.mmap_alloc(new_len);
    t.map_range(va, new_len, PTE_U | PTE_R | PTE_W);
    let copy = old_len.min(new_len);
    let mut buf: Vec<u8> = alloc::vec![0; copy];
    unsafe {
        core::ptr::copy_nonoverlapping(old as *const u8, buf.as_mut_ptr(), copy);
        core::ptr::copy_nonoverlapping(buf.as_ptr(), va as *mut u8, copy);
    }
    t.unmap_range(old, old_len);
    crate::mm::paging::flush_tlb();
    Ok(va)
}
