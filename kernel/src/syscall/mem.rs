//! Memory-related syscalls: mmap, munmap, mprotect, brk, mremap.

use crate::mm::paging;
use crate::mm::vma::{Mm, PROT_READ, PROT_WRITE, PROT_EXEC, PROT_NONE, USER_LIMIT};

const MAP_SHARED: usize = 0x01;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

fn flags_to_prot(prot: usize) -> usize {
    let mut p = PROT_NONE;
    if prot & 1 != 0 {
        p |= PROT_READ;
    }
    if prot & 2 != 0 {
        p |= PROT_WRITE;
    }
    if prot & 4 != 0 {
        p |= PROT_EXEC;
    }
    p
}

pub fn sys_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> isize {
    if len == 0 {
        return -22; // EINVAL
    }
    let len = (len + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
    let mut start = addr;
    if flags & MAP_FIXED == 0 {
        // allocate downward from mmap_next
        let t = crate::task::current();
        let mm = unsafe { &mut t.as_ref().unwrap().mm };
        start = mm.mmap_next - len;
        mm.mmap_next = start;
        // ensure no overlap with stack
        if start < crate::mm::vma::USER_MMAP_BASE / 2 {
            return -12; // ENOMEM
        }
    } else {
        start = addr;
    }
    if start + len > USER_LIMIT || start % paging::PAGE_SIZE != 0 {
        return -22;
    }
    let prot = flags_to_prot(prot);
    let t = crate::task::current();
    let mm = unsafe { &mut t.as_ref().unwrap().mm };
    if flags & MAP_ANONYMOUS != 0 {
        mm.map_anon(start, start + len, prot);
    } else {
        // file-backed: read whole file and map (private semantics)
        let t2 = crate::task::current();
        let fds = &t2.as_ref().unwrap().fds;
        let f = match fds.get(fd) {
            Some(f) => f,
            None => return -9,
        };
        let file_id = match &f.kind {
            crate::fs::FdKind::File { file_id } => *file_id,
            _ => return -22,
        };
        let file = crate::fs::fs().get(file_id).ok_or(-9).unwrap();
        let data = file.borrow().data.clone();
        mm.map_file(start, start + len, prot, &data, offset);
    }
    start as isize
}

pub fn sys_munmap(addr: usize, len: usize) -> isize {
    if len == 0 || addr % paging::PAGE_SIZE != 0 {
        return -22;
    }
    let len = (len + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
    let t = crate::task::current();
    let mm = unsafe { &mut t.as_ref().unwrap().mm };
    mm.unmap_range(addr, addr + len);
    0
}

pub fn sys_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    if len == 0 || addr % paging::PAGE_SIZE != 0 {
        return -22;
    }
    let len = (len + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
    let prot = flags_to_prot(prot);
    let t = crate::task::current();
    let mm = unsafe { &mut t.as_ref().unwrap().mm };
    mm.mprotect_range(addr, addr + len, prot);
    0
}

pub fn sys_brk(new_brk: usize) -> isize {
    let t = crate::task::current();
    let mm = unsafe { &mut t.as_ref().unwrap().mm };
    if new_brk == 0 {
        return mm.brk as isize;
    }
    let new_brk = (new_brk + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
    if new_brk < mm.brk_start || new_brk > crate::mm::vma::USER_MMAP_BASE {
        return mm.brk as isize; // fail: keep old
    }
    let old_brk = mm.brk;
    if new_brk > old_brk {
        // extend: map anon pages
        let start = (old_brk + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
        if new_brk > start {
            mm.map_anon(start, new_brk, PROT_READ | PROT_WRITE);
        }
    } else if new_brk < old_brk {
        mm.unmap_range(new_brk, old_brk);
    }
    mm.brk = new_brk;
    new_brk as isize
}

pub fn sys_mremap(addr: usize, old_len: usize, new_len: usize, flags: usize) -> isize {
    let _ = flags;
    // move the mapping to a new location (MREMAP_MAYMOVE semantics)
    let t = crate::task::current();
    let mm = unsafe { &mut t.as_ref().unwrap().mm };
    let vma = match mm.find_vma(addr) {
        Some(v) => v.clone(),
        None => return -14,
    };
    if addr != vma.start {
        return -22;
    }
    if new_len <= old_len {
        return addr as isize;
    }
    // allocate new
    let new_len_p = (new_len + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
    let new_start = mm.mmap_next - new_len_p;
    mm.mmap_next = new_start;
    // copy old data
    let old_bytes = {
        // read old pages
        let mut data = alloc::vec![0u8; old_len];
        crate::syscall::read_user(addr, old_len).unwrap_or_default();
        data
    };
    let prot = vma.prot;
    mm.map_anon(new_start, new_start + new_len_p, prot);
    let _ = crate::syscall::write_user(new_start, &old_bytes);
    // unmap old
    mm.unmap_range(addr, addr + ((old_len + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1)));
    new_start as isize
}

pub fn mm_is_writable(mm: &Mm, addr: usize) -> bool {
    mm.check_range(addr, 1, true)
}
