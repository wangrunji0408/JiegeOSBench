//! Linux riscv64 syscall dispatch.

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
        64 => sys_write(a0, a1 as *const u8, a2),           // write
        93 => sys_exit(a0 as i32),                           // exit
        94 => sys_exit(a0 as i32),                           // exit_group
        172 => crate::process::current()
            .lock()
            .as_ref()
            .map(|p| p.pid as isize)
            .unwrap_or(-1), // getpid
        214 => sys_brk(a0),                                  // brk
        226 => 0,                                            // mprotect (no-op for now)
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
    let new_end = crate::memory::frame::align_up(new, crate::memory::PAGE_SIZE);
    let mut va = crate::memory::frame::align_up(proc.brk, crate::memory::PAGE_SIZE);
    while va < new_end {
        let f = crate::memory::frame::alloc().expect("brk: out of frames");
        proc.page_table.map(va, f.0, crate::memory::page_table::USER_RW);
        va += crate::memory::PAGE_SIZE;
    }
    proc.brk = new;
    new as isize
}

fn sys_getrandom(buf: *mut u8, count: usize, _flags: usize) -> isize {
    for i in 0..count {
        unsafe { *buf.add(i) = (i * 2654435761u32).wrapping_mul(7) as u8 };
    }
    count as isize
}
