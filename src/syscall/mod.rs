//! Linux riscv64 syscall dispatch.

use crate::memory::frame::{self, PhysAddr, PAGE_SIZE};
use crate::memory::page_table;
use crate::trap::TrapContext;

// Common Linux errno values (positive; we return -errno).
pub const EPERM: isize = 1;
pub const ENOENT: isize = 2;
pub const ESRCH: isize = 3;
pub const EINTR: isize = 4;
pub const EIO: isize = 5;
pub const ENXIO: isize = 6;
pub const E2BIG: isize = 7;
pub const EBADF: isize = 9;
pub const ECHILD: isize = 10;
pub const EAGAIN: isize = 11;
pub const ENOMEM: isize = 12;
pub const EACCES: isize = 13;
pub const EFAULT: isize = 14;
pub const EBUSY: isize = 16;
pub const EEXIST: isize = 17;
pub const ENODEV: isize = 19;
pub const ENOTDIR: isize = 20;
pub const EISDIR: isize = 21;
pub const EINVAL: isize = 22;
pub const ENFILE: isize = 23;
pub const EMFILE: isize = 24;
pub const ENOSPC: isize = 28;
pub const ESPIPE: isize = 29;
pub const EROFS: isize = 30;
pub const EPIPE: isize = 32;
pub const ERANGE: isize = 34;
pub const ENOSYS: isize = 38;
pub const ENOTEMPTY: isize = 39;
pub const ENOTSOCK: isize = 88;
pub const EOPNOTSUPP: isize = 95;
pub const ECONNRESET: isize = 104;
pub const ENOBUFS: isize = 105;
pub const EISCONN: isize = 106;
pub const ENOTCONN: isize = 107;
pub const EADDRINUSE: isize = 98;

pub fn dispatch(cx: &mut TrapContext) -> isize {
    let num = cx.x[17];
    let args = [cx.x[10], cx.x[11], cx.x[12], cx.x[13], cx.x[14], cx.x[15]];
    let ret = syscall(num, args);
    ret
}

fn syscall(num: usize, args: [usize; 6]) -> isize {
    let [a0, a1, a2, a3, a4, a5] = args;
    match num {
        // basic
        29 => 0,                                             // ioctl (no-op)
        64 => sys_write(a0, a1 as *const u8, a2),           // write
        65 => sys_readv(a0, a1 as *const u8, a2),            // readv
        66 => sys_writev(a0, a1 as *const u8, a2),           // writev
        93 => sys_exit(a0 as i32),                           // exit
        94 => sys_exit(a0 as i32),                           // exit_group
        96 => sys_set_tid_address(a0),                       // set_tid_address
        99 => 0,                                             // set_robust_list
        172 => crate::process::current()
            .lock()
            .as_ref()
            .map(|p| p.pid as isize)
            .unwrap_or(-1), // getpid
        214 => sys_brk(a0),                                  // brk
        215 => sys_munmap(a0, a1),                           // munmap
        222 => sys_mmap(a0, a1, a2, a3, a4, a5),             // mmap
        226 => sys_mprotect(a0, a1, a2),                     // mprotect
        278 => sys_getrandom(a0 as *mut u8, a1, a2),         // getrandom
        _ => {
            crate::println!("[syscall] unimplemented #{num} args=[{a0:#x},{a1:#x},{a2:#x},{a3:#x}]");
            -ENOSYS
        }
    }
}

fn sys_write(fd: usize, buf: *const u8, count: usize) -> isize {
    if fd == 1 || fd == 2 {
        // stdout/stderr -> serial console
        for i in 0..count {
            crate::console::putchar(unsafe { *buf.add(i) });
        }
        count as isize
    } else {
        // TODO: other file descriptors
        -EBADF
    }
}

struct Iovec {
    base: usize,
    len: usize,
}

unsafe fn read_iovec(ptr: *const u8, idx: usize) -> Iovec {
    let base = *(ptr.add(idx * 16) as *const usize);
    let len = *(ptr.add(idx * 16 + 8) as *const usize);
    Iovec { base, len }
}

fn sys_writev(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let v = unsafe { read_iovec(iov, i) };
        if fd == 1 || fd == 2 {
            for j in 0..v.len {
                crate::console::putchar(unsafe { *(v.base as *const u8).add(j) });
            }
            total += v.len;
        } else {
            return -EBADF;
        }
    }
    total as isize
}

fn sys_readv(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    if fd != 0 {
        return -EBADF;
    }
    let mut total = 0usize;
    for i in 0..iovcnt {
        let v = unsafe { read_iovec(iov, i) };
        for j in 0..v.len {
            if let Some(c) = crate::console::getchar() {
                unsafe { *(v.base as *mut u8).add(j) = c };
                total += 1;
            } else {
                return total as isize;
            }
        }
    }
    total as isize
}

fn sys_exit(code: i32) -> isize {
    crate::println!("[process] exit({code})");
    crate::sbi::shutdown();
}

fn sys_brk(addr: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("brk: no process");
    if addr == 0 {
        return proc.brk as isize;
    }
    let new = addr;
    if new < proc.brk {
        // shrink (ignore for now)
        proc.brk = new;
        return new as isize;
    }
    // grow: map frames up to aligned new brk
    let new_end = frame::align_up(new, PAGE_SIZE);
    let mut va = frame::align_up(proc.brk, PAGE_SIZE);
    while va < new_end {
        let f = frame::alloc().expect("brk: out of frames");
        proc.page_table.map(va, f.0, page_table::USER_RW);
        va += PAGE_SIZE;
    }
    proc.brk = new;
    new as isize
}

fn prot_to_flags(prot: usize) -> usize {
    let mut flags = page_table::PTE_U | page_table::PTE_A | page_table::PTE_D;
    if prot & 1 != 0 { flags |= page_table::PTE_R; }
    if prot & 2 != 0 { flags |= page_table::PTE_W; }
    if prot & 4 != 0 { flags |= page_table::PTE_X; }
    flags
}

fn sys_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: usize, offset: usize) -> isize {
    if len == 0 {
        return -EINVAL;
    }
    let len_aligned = frame::align_up(len, PAGE_SIZE);
    let fixed = flags & 0x10 != 0;
    let anonymous = flags & 0x20 != 0;

    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("mmap: no process");

    let start = if fixed {
        frame::align_down(addr, PAGE_SIZE)
    } else if addr != 0 {
        frame::align_up(addr, PAGE_SIZE)
    } else {
        proc.mmap_hint
    };

    let pte = prot_to_flags(prot);
    if pte & (page_table::PTE_R | page_table::PTE_W | page_table::PTE_X) == 0 {
        return -EACCES;
    }

    let mut va = start;
    while va < start + len_aligned {
        let f = match frame::alloc() {
            Some(f) => f,
            None => return -ENOMEM,
        };
        proc.page_table.map(va, f.0, pte);
        // File-backed mapping: copy from fd at offset.
        if !anonymous && fd != usize::MAX {
            // TODO: read from file descriptor
        }
        va += PAGE_SIZE;
    }
    proc.mmap_hint = start + len_aligned;
    start as isize
}

fn sys_munmap(addr: usize, len: usize) -> isize {
    if len == 0 {
        return 0;
    }
    let start = frame::align_down(addr, PAGE_SIZE);
    let end = frame::align_up(addr + len, PAGE_SIZE);
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("munmap: no process");
    let mut va = start;
    while va < end {
        if let Some(pa) = proc.page_table.translate(va) {
            proc.page_table.unmap(va);
            frame::dealloc(PhysAddr(frame::align_down(pa, PAGE_SIZE)));
        }
        va += PAGE_SIZE;
    }
    0
}

fn sys_mprotect(addr: usize, len: usize, prot: usize) -> isize {
    if len == 0 {
        return 0;
    }
    let start = frame::align_down(addr, PAGE_SIZE);
    let end = frame::align_up(addr + len, PAGE_SIZE);
    let pte = prot_to_flags(prot);
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("mprotect: no process");
    let mut va = start;
    while va < end {
        match proc.page_table.translate(va) {
            Some(pa) => {
                proc.page_table.map(va, frame::align_down(pa, PAGE_SIZE), pte);
            }
            None => return -ENOMEM,
        }
        va += PAGE_SIZE;
    }
    0
}

fn sys_getrandom(buf: *mut u8, count: usize, _flags: usize) -> isize {
    for i in 0..count {
        unsafe { *buf.add(i) = ((i.wrapping_mul(1103515245).wrapping_add(12345)) >> 16) as u8 };
    }
    count as isize
}

fn sys_set_tid_address(tidptr: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("set_tid_address: no process");
    proc.clear_child_tid = tidptr;
    proc.pid as isize
}
