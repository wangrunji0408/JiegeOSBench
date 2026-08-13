//! Linux riscv64 syscall dispatch.

use crate::memory::frame::{self, PhysAddr, PAGE_SIZE};
use crate::memory::page_table;
use crate::trap::TrapContext;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::fs::{FileDesc, FileKind};

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
    crate::println!("[sc] #{} -> {:#x}", num, ret);
    ret
}

fn syscall(num: usize, args: [usize; 6]) -> isize {
    let [a0, a1, a2, a3, a4, a5] = args;
    match num {
        // basic
        17 => sys_getcwd(a0 as *mut u8, a1),                  // getcwd
        23 => sys_dup(a0),                                     // dup
        24 => sys_dup3(a0, a1, a2),                            // dup3
        25 => sys_fcntl(a0, a1, a2),                           // fcntl
        29 => sys_ioctl(a0, a1, a2),                           // ioctl
        34 => sys_mkdirat(a0, a1 as *const u8, a2),            // mkdirat
        35 => sys_unlinkat(a0, a1 as *const u8, a2),           // unlinkat
        38 => sys_renameat(a0, a1 as *const u8, a2, a3 as *const u8), // renameat
        48 => sys_faccessat(a0, a1 as *const u8, a2),          // faccessat
        49 => sys_chdir(a0 as *const u8),                      // chdir
        52 | 53 | 54 | 55 => 0,                                // fchmod/fchmodat/fchownat/fchown (no-op)
        56 => sys_openat(a0, a1 as *const u8, a2, a3),         // openat
        57 => sys_close(a0),                                   // close
        61 => sys_getdents64(a0, a1 as *mut u8, a2),           // getdents64
        62 => sys_lseek(a0, a1 as i64, a2),                    // lseek
        63 => sys_read(a0, a1 as *mut u8, a2),                 // read
        64 => sys_write(a0, a1 as *const u8, a2),              // write
        65 => sys_readv(a0, a1 as *const u8, a2),              // readv
        66 => sys_writev(a0, a1 as *const u8, a2),             // writev
        67 => sys_pread64(a0, a1 as *mut u8, a2, a3),          // pread64
        68 => sys_pwrite64(a0, a1 as *const u8, a2, a3),       // pwrite64
        73 => sys_ppoll(a0 as *const u8, a1, a2),              // ppoll
        198 => sys_socket(a0, a1, a2),                         // socket
        200 => sys_bind(a0, a1 as *const u8, a2),              // bind
        201 => sys_listen(a0, a1),                             // listen
        202 => sys_accept(a0, a1 as *mut u8, a2 as *mut u8),   // accept
        203 => sys_connect(a0, a1 as *const u8, a2),           // connect
        204 => sys_getsockname(a0, a1 as *mut u8, a2 as *mut u8), // getsockname
        205 => sys_getpeername(a0, a1 as *mut u8, a2 as *mut u8), // getpeername
        206 => sys_sendto(a0, a1 as *const u8, a2, a3, a4 as *const u8, a5), // sendto
        207 => sys_recvfrom(a0, a1 as *mut u8, a2, a3, a4 as *mut u8, a5 as *mut u8), // recvfrom
        208 | 209 => 0,                                       // setsockopt/getsockopt (no-op)
        210 => 0,                                             // shutdown (no-op)
        211 => sys_sendmsg(a0, a1 as *const u8, a2),          // sendmsg
        212 => sys_recvmsg(a0, a1 as *mut u8, a2),            // recvmsg
        242 => sys_accept(a0, a1 as *mut u8, a2 as *mut u8),  // accept4
        78 => sys_readlinkat(a0, a1 as *const u8, a2 as *mut u8, a3), // readlinkat
        79 => sys_newfstatat(a0, a1 as *const u8, a2 as *mut u8, a3), // newfstatat
        80 => sys_fstat(a0, a1 as *mut u8),                    // fstat
        93 => sys_exit(a0 as i32),                             // exit
        94 => sys_exit(a0 as i32),                           // exit_group
        96 => sys_set_tid_address(a0),                       // set_tid_address
        98 => 0,                                             // futex
        99 => 0,                                             // set_robust_list
        101 => 0,                                            // nanosleep
        113 => sys_clock_gettime(a0, a1 as *mut u8),         // clock_gettime
        115 => 0,                                            // clock_nanosleep
        122 => 0,                                            // sched_setaffinity
        123 => sys_sched_getaffinity(a0, a1, a2),            // sched_getaffinity
        124 => 0,                                            // sched_yield
        129 | 130 | 131 => 0,                                // kill/tkill/tgkill
        132 => 0,                                            // sigaltstack
        134 => 0,                                            // rt_sigaction
        135 => 0,                                            // rt_sigprocmask
        139 => 0,                                            // rt_sigreturn
        153 => sys_times(a0 as *mut u8),                     // times
        157 => crate::process::current().lock().as_ref().map(|p| p.pid as isize).unwrap_or(1), // setsid
        160 => sys_uname(a0 as *mut u8),                     // uname
        163 => sys_getrlimit(a0, a1 as *mut u8),             // getrlimit
        164 => 0,                                            // setrlimit
        165 => sys_zero_buf(a1 as *mut u8, 144),             // getrusage
        166 => 0o22,                                         // umask
        167 => 0,                                            // prctl
        169 => sys_gettimeofday(a0 as *mut u8, a1),          // gettimeofday
        173 => 1,                                            // getppid
        174 | 175 | 176 | 177 => 0,                          // getuid/euid/gid/egid
        178 => crate::process::current().lock().as_ref().map(|p| p.pid as isize).unwrap_or(1), // gettid
        179 => sys_sysinfo(a0 as *mut u8),                   // sysinfo
        232 => 0,                                            // mincore
        233 => 0,                                            // madvise
        261 => 0,                                            // prlimit64
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
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("write: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    fd_write(f, buf, count)
}

/// Write `count` bytes from `buf` to the open file `f`.
fn fd_write(f: &mut FileDesc, buf: *const u8, count: usize) -> isize {
    match &f.kind {
        FileKind::Stdout | FileKind::Stderr => {
            for i in 0..count {
                crate::console::putchar(unsafe { *buf.add(i) });
            }
            count as isize
        }
        FileKind::Null => count as isize,
        FileKind::Inode(node) => {
            let node = node.clone();
            let mut data = node.data.lock();
            let start = if f.flags & crate::fs::O_APPEND != 0 { data.len() } else { f.offset.min(data.len()) };
            let end = start + count;
            if end > data.len() {
                data.resize(end, 0);
            }
            unsafe { core::ptr::copy_nonoverlapping(buf, data.as_mut_ptr().add(start), count); }
            f.offset = end;
            count as isize
        }
        _ => -EBADF,
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
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("writev: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    for i in 0..iovcnt {
        let v = unsafe { read_iovec(iov, i) };
        let n = fd_write(f, v.base as *const u8, v.len);
        if n < 0 {
            return if total > 0 { total as isize } else { n };
        }
        total += n as usize;
    }
    total as isize
}

fn sys_readv(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    let mut total = 0usize;
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("readv: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    for i in 0..iovcnt {
        let v = unsafe { read_iovec(iov, i) };
        let n = fd_read(f, v.base as *mut u8, v.len);
        if n < 0 {
            return if total > 0 { total as isize } else { n };
        }
        total += n as usize;
        if (n as usize) < v.len {
            break;
        }
    }
    total as isize
}

fn sys_read(fd: usize, buf: *mut u8, count: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("read: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    fd_read(f, buf, count)
}

/// Read up to `count` bytes from open file `f` into `buf`.
fn fd_read(f: &mut FileDesc, buf: *mut u8, count: usize) -> isize {
    match &f.kind {
        FileKind::Stdin => {
            let mut n = 0;
            while n < count {
                match crate::console::getchar() {
                    Some(c) => {
                        unsafe { *buf.add(n) = c };
                        n += 1;
                    }
                    None => break,
                }
            }
            n as isize
        }
        FileKind::Null => 0,
        FileKind::Inode(node) => {
            let node = node.clone();
            let data = node.data.lock();
            let start = f.offset.min(data.len());
            let avail = data.len() - start;
            let n = avail.min(count);
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr().add(start), buf, n); }
            f.offset += n;
            n as isize
        }
        _ => -EBADF,
    }
}

/// Read a NUL-terminated string from user memory (SUM=1 is active during syscalls).
unsafe fn read_cstr(ptr: *const u8) -> String {
    let mut s = String::new();
    let mut p = ptr;
    let mut guard = 0;
    while *p != 0 && guard < 8192 {
        s.push(*p as char);
        p = p.add(1);
        guard += 1;
    }
    s
}

const AT_FDCWD: usize = (-100isize) as usize;

fn resolve_path(proc: &crate::process::Process, dirfd: usize, path: &str) -> String {
    if path.starts_with('/') {
        String::from(path)
    } else {
        let base = if dirfd == AT_FDCWD {
            proc.cwd.clone()
        } else {
            String::from("/")
        };
        if base == "/" {
            alloc::format!("/{}", path)
        } else {
            alloc::format!("{}/{}", base, path)
        }
    }
}

fn alloc_fd(proc: &mut crate::process::Process, fd: FileDesc) -> isize {
    for (i, slot) in proc.fds.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(fd);
            return i as isize;
        }
    }
    proc.fds.push(Some(fd));
    (proc.fds.len() - 1) as isize
}

fn sys_openat(dirfd: usize, path_ptr: *const u8, flags: usize, _mode: usize) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("openat: no process");
    let full = resolve_path(proc, dirfd, &path);
    crate::println!("[openat] \"{}\"", full);

    let accmode = flags & 0x3;
    let readable = accmode != 1; // not write-only
    let writable = accmode != 0; // not read-only
    let fl = flags as u32;

    // Special files.
    match full.as_str() {
        "/dev/null" => return alloc_fd(proc, FileDesc { kind: FileKind::Null, offset: 0, flags: flags as u32, readable, writable }),
        "/dev/stdin" => return alloc_fd(proc, FileDesc { kind: FileKind::Stdin, offset: 0, flags: flags as u32, readable, writable }),
        "/dev/stdout" => return alloc_fd(proc, FileDesc { kind: FileKind::Stdout, offset: 0, flags: flags as u32, readable, writable }),
        "/dev/stderr" => return alloc_fd(proc, FileDesc { kind: FileKind::Stderr, offset: 0, flags: flags as u32, readable, writable }),
        _ => {}
    }

    let node = if fl & crate::fs::O_CREAT != 0 {
        match crate::fs::lookup(&full) {
            Some(n) => n,
            None => match crate::fs::create_file(&full) {
                Some(n) => n,
                None => return -ENOENT,
            },
        }
    } else {
        match crate::fs::lookup(&full) {
            Some(n) => n,
            None => return -ENOENT,
        }
    };

    if fl & crate::fs::O_DIRECTORY != 0 && !node.is_dir {
        return -ENOTDIR;
    }
    if fl & crate::fs::O_TRUNC != 0 && !node.is_dir {
        node.data.lock().clear();
    }

    let offset = if fl & crate::fs::O_APPEND != 0 { node.data.lock().len() } else { 0 };
    let fd = FileDesc {
        kind: FileKind::Inode(node),
        offset,
        flags: flags as u32,
        readable,
        writable,
    };
    alloc_fd(proc, fd)
}

fn sys_close(fd: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("close: no process");
    if fd >= proc.fds.len() || proc.fds[fd].is_none() {
        return -EBADF;
    }
    proc.fds[fd] = None;
    0
}

fn sys_lseek(fd: usize, offset: i64, whence: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("lseek: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    let size = match &f.kind {
        FileKind::Inode(node) => node.data.lock().len() as i64,
        _ => 0,
    };
    let base = match whence {
        0 => 0,
        1 => f.offset as i64,
        2 => size,
        _ => return -EINVAL,
    };
    let new = base + offset;
    if new < 0 {
        return -EINVAL;
    }
    f.offset = new as usize;
    new as isize
}

fn fill_stat(node: &Arc<crate::fs::INode>, buf: *mut u8) {
    let mut st = [0u8; 128];
    crate::fs::stat_of(node, &mut st);
    unsafe { core::ptr::copy_nonoverlapping(st.as_ptr(), buf, 128); }
}

fn fill_char_stat(buf: *mut u8) {
    let mut st = [0u8; 128];
    let w = |off: usize, bytes: &[u8], st: &mut [u8; 128]| st[off..off + bytes.len()].copy_from_slice(bytes);
    w(16, &(0o020666u32).to_le_bytes(), &mut st); // S_IFCHR | 0666
    w(20, &1u32.to_le_bytes(), &mut st);
    let t = crate::sbi::get_time() as i64;
    w(72, &t.to_le_bytes(), &mut st);
    w(88, &t.to_le_bytes(), &mut st);
    w(104, &t.to_le_bytes(), &mut st);
    unsafe { core::ptr::copy_nonoverlapping(st.as_ptr(), buf, 128); }
}

fn sys_fstat(fd: usize, buf: *mut u8) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("fstat: no process");
    let f = match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => f,
        None => return -EBADF,
    };
    match &f.kind {
        FileKind::Inode(node) => fill_stat(node, buf),
        _ => fill_char_stat(buf),
    }
    0
}

fn sys_newfstatat(dirfd: usize, path_ptr: *const u8, buf: *mut u8, _flags: usize) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("newfstatat: no process");
    let full = resolve_path(proc, dirfd, &path);
    match crate::fs::lookup(&full) {
        Some(node) => {
            fill_stat(&node, buf);
            0
        }
        None => -ENOENT,
    }
}

fn sys_getdents64(fd: usize, dirp: *mut u8, count: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("getdents64: no process");
    let f = match proc.fds.get_mut(fd).and_then(Option::as_mut) {
        Some(f) => f,
        None => return -EBADF,
    };
    let node = match &f.kind {
        FileKind::Inode(n) if n.is_dir => n.clone(),
        _ => return -ENOTDIR,
    };
    let children = node.children.lock();
    let mut written = 0usize;
    let mut idx = f.offset;
    while idx < children.len() {
        let child = &children[idx];
        let name = child.name.as_bytes();
        let reclen = 19 + name.len() + 1; // d_ino(8) + d_off(8) + d_reclen(2) + d_type(1) + name + nul
        let reclen = (reclen + 7) & !7; // align to 8
        if written + reclen > count {
            break;
        }
        let d_type = if child.is_dir { 4u8 } else { 8u8 };
        unsafe {
            let p = dirp.add(written);
            (p as *mut u64).write_unaligned(idx as u64 + 1); // d_ino
            (p.add(8) as *mut i64).write_unaligned((idx + 1) as i64); // d_off
            (p.add(16) as *mut u16).write_unaligned(reclen as u16); // d_reclen
            (p.add(18) as *mut u8).write(d_type); // d_type
            core::ptr::copy_nonoverlapping(name.as_ptr(), p.add(19), name.len());
            p.add(19 + name.len()).write(0);
        }
        written += reclen;
        idx += 1;
    }
    f.offset = idx;
    written as isize
}

fn sys_ioctl(fd: usize, _req: usize, _arg: usize) -> isize {
    // Validate the fd; accept all ioctls for now (mostly harmless no-ops).
    let cur = crate::process::current().lock();
    let proc = cur.as_ref().expect("ioctl: no process");
    if fd >= proc.fds.len() || proc.fds[fd].is_none() {
        return -EBADF;
    }
    0
}

const F_DUPFD: usize = 0;
const F_GETFD: usize = 1;
const F_SETFD: usize = 2;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;

fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("fcntl: no process");
    match cmd {
        F_GETFL => match proc.fds.get(fd).and_then(Option::as_ref) {
            Some(f) => (f.flags & 0o7777) as isize,
            None => -EBADF,
        },
        F_SETFL => match proc.fds.get_mut(fd).and_then(Option::as_mut) {
            Some(f) => {
                f.flags = (f.flags & !0o7777) | (arg as u32 & 0o7777);
                0
            }
            None => -EBADF,
        },
        F_GETFD | F_SETFD => 0,
        F_DUPFD | 1030 => {
            // F_DUPFD / F_DUPFD_CLOEXEC
            let f = match proc.fds.get(fd).and_then(Option::as_ref) {
                Some(f) => f,
                None => return -EBADF,
            };
            let kind = f.kind.clone();
            let newfd = FileDesc { kind, offset: f.offset, flags: f.flags, readable: f.readable, writable: f.writable };
            let mut target = arg;
            while target < proc.fds.len() && proc.fds[target].is_some() {
                target += 1;
            }
            if target == proc.fds.len() {
                proc.fds.push(Some(newfd));
            } else {
                proc.fds[target] = Some(newfd);
            }
            target as isize
        }
        _ => 0,
    }
}

fn sys_dup(fd: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("dup: no process");
    let f = match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => f,
        None => return -EBADF,
    };
    let newfd = FileDesc { kind: f.kind.clone(), offset: f.offset, flags: f.flags, readable: f.readable, writable: f.writable };
    alloc_fd(proc, newfd)
}

fn sys_dup3(fd: usize, newfd: usize, _flags: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("dup3: no process");
    let f = match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => f,
        None => return -EBADF,
    };
    let new = FileDesc { kind: f.kind.clone(), offset: f.offset, flags: f.flags, readable: f.readable, writable: f.writable };
    if newfd >= proc.fds.len() {
        proc.fds.resize(newfd + 1, None);
    }
    proc.fds[newfd] = Some(new);
    newfd as isize
}

fn sys_unlinkat(dirfd: usize, path_ptr: *const u8, _flags: usize) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("unlinkat: no process");
    let full = resolve_path(proc, dirfd, &path);
    crate::fs::unlink(&full)
}

fn sys_mkdirat(dirfd: usize, path_ptr: *const u8, _mode: usize) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("mkdirat: no process");
    let full = resolve_path(proc, dirfd, &path);
    crate::fs::mkdir(&full)
}

fn sys_renameat(_olddir: usize, old_ptr: *const u8, _newdir: usize, new_ptr: *const u8) -> isize {
    let old = unsafe { read_cstr(old_ptr) };
    let new = unsafe { read_cstr(new_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("renameat: no process");
    let old_full = resolve_path(proc, AT_FDCWD, &old);
    let new_full = resolve_path(proc, AT_FDCWD, &new);
    let node = match crate::fs::lookup(&old_full) {
        Some(n) => n,
        None => return -ENOENT,
    };
    let data = node.data.lock().clone();
    let is_dir = node.is_dir;
    let _ = crate::fs::unlink(&old_full);
    if is_dir {
        crate::fs::mkdir(&new_full)
    } else {
        match crate::fs::create_file(&new_full) {
            Some(n) => {
                *n.data.lock() = data;
                0
            }
            None => -ENOENT,
        }
    }
}

fn sys_faccessat(dirfd: usize, path_ptr: *const u8, _mode: usize) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("faccessat: no process");
    let full = resolve_path(proc, dirfd, &path);
    if crate::fs::lookup(&full).is_some() {
        0
    } else {
        -ENOENT
    }
}

fn sys_chdir(path_ptr: *const u8) -> isize {
    let path = unsafe { read_cstr(path_ptr) };
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("chdir: no process");
    let full = resolve_path(proc, AT_FDCWD, &path);
    match crate::fs::lookup(&full) {
        Some(n) if n.is_dir => {
            proc.cwd = full;
            0
        }
        Some(_) => -ENOTDIR,
        None => -ENOENT,
    }
}

fn sys_getcwd(buf: *mut u8, size: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("getcwd: no process");
    let cwd = proc.cwd.clone();
    if cwd.len() + 1 > size {
        return -ERANGE;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf, cwd.len());
        *buf.add(cwd.len()) = 0;
    }
    cwd.len() as isize
}

fn sys_readlinkat(_dirfd: usize, _path: *const u8, _buf: *mut u8, _size: usize) -> isize {
    -EINVAL
}

fn sys_pread64(fd: usize, buf: *mut u8, count: usize, offset: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("pread64: no process");
    let f = match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => f,
        None => return -EBADF,
    };
    match &f.kind {
        FileKind::Inode(node) => {
            let node = node.clone();
            let data = node.data.lock();
            let start = offset.min(data.len());
            let avail = data.len() - start;
            let n = avail.min(count);
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr().add(start), buf, n); }
            n as isize
        }
        _ => -EBADF,
    }
}

fn sys_pwrite64(fd: usize, buf: *const u8, count: usize, offset: usize) -> isize {
    let mut cur = crate::process::current().lock();
    let proc = cur.as_mut().expect("pwrite64: no process");
    let f = match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => f,
        None => return -EBADF,
    };
    match &f.kind {
        FileKind::Inode(node) => {
            let node = node.clone();
            let mut data = node.data.lock();
            let end = offset + count;
            if end > data.len() {
                data.resize(end, 0);
            }
            unsafe { core::ptr::copy_nonoverlapping(buf, data.as_mut_ptr().add(offset), count); }
            count as isize
        }
        _ => -EBADF,
    }
}

fn sys_exit(code: i32) -> isize {
    crate::println!("[process] exit({code})");
    crate::sbi::shutdown();
}

fn fd_socket_handle(fd: usize) -> Option<usize> {
    let cur = crate::process::current().lock();
    let proc = cur.as_ref().unwrap();
    match proc.fds.get(fd).and_then(Option::as_ref) {
        Some(f) => match &f.kind {
            FileKind::Socket(h) => Some(*h),
            _ => None,
        },
        None => None,
    }
}

unsafe fn parse_sockaddr_in(ptr: *const u8) -> (u32, u16) {
    let port = u16::from_be(*(ptr.add(2) as *const u16));
    let addr_be = *(ptr.add(4) as *const u32);
    (addr_be, port)
}

fn write_sockaddr_in(ptr: *mut u8, addr: u32, port: u16) {
    unsafe {
        (ptr as *mut u16).write(2); // AF_INET
        (ptr.add(2) as *mut u16).write(port.to_be());
        (ptr.add(4) as *mut u32).write(addr);
        core::ptr::write_bytes(ptr.add(8), 0, 8);
    }
}

fn sys_socket(domain: usize, ty: usize, _proto: usize) -> isize {
    let _ = domain;
    let is_tcp = ty == 1; // SOCK_STREAM
    match crate::net::socket_new(is_tcp) {
        Ok(h) => {
            let mut cur = crate::process::current().lock();
            let proc = cur.as_mut().unwrap();
            let fd = FileDesc {
                kind: FileKind::Socket(h),
                offset: 0,
                flags: crate::fs::O_RDWR as u32,
                readable: true,
                writable: true,
            };
            alloc_fd(proc, fd)
        }
        Err(e) => e,
    }
}

fn sys_bind(fd: usize, addr: *const u8, _addrlen: usize) -> isize {
    let (_addr, port) = unsafe { parse_sockaddr_in(addr) };
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    crate::net::socket_bind(h, port)
}

fn sys_listen(fd: usize, _backlog: usize) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    crate::net::socket_listen_stored(h)
}

fn sys_accept(fd: usize, addr: *mut u8, addrlen: *mut u8) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    match crate::net::socket_accept(h) {
        Ok((conn_handle, new_listen_handle, peer, port)) => {
            let mut cur = crate::process::current().lock();
            let proc = cur.as_mut().unwrap();
            // Update the listening fd to point at the new listening socket.
            if let Some(Some(f)) = proc.fds.get_mut(fd) {
                if let FileKind::Socket(ref mut sh) = f.kind {
                    *sh = new_listen_handle;
                }
            }
            // Create a new fd for the accepted connection.
            let conn = FileDesc {
                kind: FileKind::Socket(conn_handle),
                offset: 0,
                flags: crate::fs::O_RDWR as u32,
                readable: true,
                writable: true,
            };
            let newfd = alloc_fd(proc, conn);
            if addr != 0 {
                write_sockaddr_in(addr, u32::from_be_bytes(peer), port);
            }
            if addrlen != 0 {
                unsafe { (addrlen as *mut u32).write(16); }
            }
            newfd
        }
        Err(e) => e,
    }
}

fn sys_connect(fd: usize, addr: *const u8, _addrlen: usize) -> isize {
    let (addr_be, port) = unsafe { parse_sockaddr_in(addr) };
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    let a = (addr_be >> 24) as u8;
    let b = (addr_be >> 16) as u8;
    let c = (addr_be >> 8) as u8;
    let d = addr_be as u8;
    crate::net::socket_connect(h, a, b, c, d, port)
}

fn sys_getsockname(fd: usize, addr: *mut u8, addrlen: *mut u8) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    match crate::net::socket_local(h) {
        Some((a, p)) => {
            if addr != 0 {
                write_sockaddr_in(addr, a, p);
            }
            if addrlen != 0 {
                unsafe { (addrlen as *mut u32).write(16); }
            }
            0
        }
        None => -EINVAL,
    }
}

fn sys_getpeername(fd: usize, addr: *mut u8, addrlen: *mut u8) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    match crate::net::socket_peer(h) {
        Some((a, p)) => {
            if addr != 0 {
                write_sockaddr_in(addr, a, p);
            }
            if addrlen != 0 {
                unsafe { (addrlen as *mut u32).write(16); }
            }
            0
        }
        None => -ENOTCONN,
    }
}

fn sys_sendto(fd: usize, buf: *const u8, len: usize, _flags: usize, _to: *const u8, _tolen: usize) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    let data = unsafe { core::slice::from_raw_parts(buf, len) };
    crate::net::socket_send(h, data)
}

fn sys_recvfrom(fd: usize, buf: *mut u8, len: usize, _flags: usize, _from: *mut u8, _fromlen: *mut u8) -> isize {
    let h = match fd_socket_handle(fd) {
        Some(h) => h,
        None => return -EBADF,
    };
    let data = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    crate::net::socket_recv(h, data)
}

fn sys_sendmsg(fd: usize, msg: *const u8, _flags: usize) -> isize {
    // struct msghdr: name(8) namelen(4) iov(8) iovlen(4) control(8) controllen(4) flags(4)
    let iov = unsafe { *(msg.add(16) as *const usize) };
    let iovcnt = unsafe { *(msg.add(24) as *const u32) } as usize;
    sys_writev(fd, iov as *const u8, iovcnt)
}

fn sys_recvmsg(fd: usize, msg: *mut u8, _flags: usize) -> isize {
    let iov = unsafe { *(msg.add(16) as *const usize) };
    let iovcnt = unsafe { *(msg.add(24) as *const u32) } as usize;
    sys_readv(fd, iov as *const u8, iovcnt)
}

fn sys_ppoll(fds: *const u8, nfds: usize, _timeout: usize) -> isize {
    crate::net::poll();
    let mut ready = 0;
    for i in 0..nfds {
        let p = unsafe { fds.add(i * 8) };
        let fd = unsafe { *(p as *const i32) } as usize;
        let events = unsafe { *(p.add(4) as *const i16) };
        let mut revents = 0i16;
        if (fd as i32) >= 0 {
            let cur = crate::process::current().lock();
            let proc = cur.as_ref().unwrap();
            if let Some(Some(f)) = proc.fds.get(fd) {
                match &f.kind {
                    FileKind::Socket(h) => {
                        if events & 1 != 0 && crate::net::socket_readable(*h) {
                            revents |= 1; // POLLIN
                        }
                        if events & 4 != 0 && crate::net::socket_writable(*h) {
                            revents |= 4; // POLLOUT
                        }
                    }
                    FileKind::Stdin => {
                        if events & 1 != 0 && crate::console::getchar().is_some() {
                            revents |= 1;
                        }
                    }
                    _ => {
                        // regular files / null are always ready
                        revents |= events & (1 | 4);
                    }
                }
            }
        }
        unsafe { *(p.add(6) as *mut i16) = revents; }
        if revents != 0 {
            ready += 1;
        }
    }
    ready as isize
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

fn sys_clock_gettime(clk_id: usize, tp: *mut u8) -> isize {
    let ns = crate::sbi::get_time();
    let (sec, nsec) = (ns / 1_000_000_000, ns % 1_000_000_000);
    unsafe {
        (tp as *mut i64).write(sec as i64);
        (tp.add(8) as *mut i64).write(nsec as i64);
    }
    let _ = clk_id;
    0
}

fn sys_gettimeofday(tv: *mut u8, _tz: usize) -> isize {
    let ns = crate::sbi::get_time();
    let (sec, usec) = (ns / 1_000_000_000, (ns % 1_000_000_000) / 1000);
    unsafe {
        (tv as *mut i64).write(sec as i64);
        (tv.add(8) as *mut i64).write(usec as i64);
    }
    0
}

fn write_cfield(buf: *mut u8, off: usize, s: &str) {
    unsafe {
        let dst = buf.add(off);
        core::ptr::copy_nonoverlapping(s.as_ptr(), dst, s.len());
        *dst.add(s.len()) = 0;
    }
}

fn sys_uname(buf: *mut u8) -> isize {
    write_cfield(buf, 0, "Linux");
    write_cfield(buf, 65, "ijiege");
    write_cfield(buf, 130, "6.6.0-iJiege");
    write_cfield(buf, 195, "#1");
    write_cfield(buf, 260, "riscv64");
    write_cfield(buf, 325, "(none)");
    0
}

fn sys_getrlimit(res: usize, rlim: *mut u8) -> isize {
    const RLIM_INFINITY: u64 = u64::MAX;
    let (cur, max) = match res {
        7 => (1024, 4096),        // RLIMIT_NOFILE
        8 => (RLIM_INFINITY, RLIM_INFINITY), // RLIMIT_AS
        6 => (RLIM_INFINITY, RLIM_INFINITY), // RLIMIT_CORE
        _ => (RLIM_INFINITY, RLIM_INFINITY),
    };
    unsafe {
        (rlim as *mut u64).write(cur);
        (rlim.add(8) as *mut u64).write(max);
    }
    0
}

fn sys_sched_getaffinity(_pid: usize, _len: usize, mask: usize) -> isize {
    // Single CPU: set the first bit.
    unsafe { (mask as *mut u64).write(1); }
    8
}

fn sys_times(buf: *mut u8) -> isize {
    sys_zero_buf(buf, 32)
}

fn sys_sysinfo(buf: *mut u8) -> isize {
    sys_zero_buf(buf, 112);
    let ns = crate::sbi::get_time();
    unsafe {
        (buf as *mut i64).write((ns / 1_000_000_000) as i64); // uptime seconds
        (buf.add(104) as *mut u32).write(4096); // mem_unit
    }
    0
}

fn sys_zero_buf(buf: *mut u8, len: usize) -> isize {
    unsafe { core::ptr::write_bytes(buf, 0, len); }
    0
}
