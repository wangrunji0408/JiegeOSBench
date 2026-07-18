use crate::mm::{translated_byte_buffer, MapPermission, VirtAddr};
use crate::task::{current_task, current_user_token};

const PROT_READ: usize = 1;
const PROT_WRITE: usize = 2;
const PROT_EXEC: usize = 4;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

fn prot_to_perm(prot: usize) -> MapPermission {
    let mut perm = MapPermission::U;
    if prot & PROT_READ != 0 {
        perm |= MapPermission::R;
    }
    if prot & PROT_WRITE != 0 {
        perm |= MapPermission::W;
    }
    if prot & PROT_EXEC != 0 {
        perm |= MapPermission::X;
    }
    perm
}

pub fn sys_brk(new_brk: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    if new_brk == 0 {
        return inner.program_brk as isize;
    }
    let heap_bottom = inner.heap_bottom;
    inner
        .memory_set
        .adjust_heap(VirtAddr(heap_bottom), VirtAddr(new_brk));
    inner.program_brk = new_brk;
    new_brk as isize
}

pub fn sys_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: isize, offset: usize) -> isize {
    if len == 0 {
        return -22; // EINVAL
    }
    let perm = prot_to_perm(prot);
    let fixed = if flags & MAP_FIXED != 0 { Some(addr) } else { None };

    let file = if flags & MAP_ANONYMOUS == 0 && fd >= 0 {
        let task = current_task().unwrap();
        let f = task.inner_lock().get_fd(fd as usize);
        f
    } else {
        None
    };

    let base = {
        let task = current_task().unwrap();
        let mut inner = task.inner_lock();
        inner.memory_set.mmap(fixed, len, perm)
    };

    if let Some(file) = file {
        let token = current_user_token();
        let copy_len = len.min(file.size().saturating_sub(offset));
        if copy_len > 0 {
            let buffers = translated_byte_buffer(token, base as *const u8, copy_len);
            let mut off = offset;
            for b in buffers {
                let n = file.read_at(off, b);
                off += n;
                if n < b.len() {
                    break;
                }
            }
        }
    }
    base as isize
}

pub fn sys_munmap(addr: usize, len: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    if inner.memory_set.munmap(addr, len) {
        0
    } else {
        -22
    }
}

pub fn sys_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    let perm = prot_to_perm(prot);
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    inner.memory_set.mprotect(addr, len, perm);
    0
}
