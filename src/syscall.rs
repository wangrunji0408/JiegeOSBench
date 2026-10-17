// Linux (asm-generic / riscv64) syscall emulation layer.
use crate::arch::TrapFrame;
use crate::fs;
use crate::mm;
use crate::net;
use crate::println;
use crate::syscall_nr::*;
use alloc::vec::Vec;

pub const EPERM: isize = 1;
pub const ENOENT: isize = 2;
pub const ESRCH: isize = 3;
pub const EINTR: isize = 4;
pub const EIO: isize = 5;
pub const ENXIO: isize = 6;
pub const E2BIG: isize = 7;
pub const ENOEXEC: isize = 8;
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
pub const ENOTTY: isize = 25;
pub const EFBIG: isize = 27;
pub const ENOSPC: isize = 28;
pub const ESPIPE: isize = 29;
pub const EROFS: isize = 30;
pub const EPIPE: isize = 32;
pub const ERANGE: isize = 34;
pub const ENOSYS: isize = 38;
pub const ENOTEMPTY: isize = 39;
pub const ELOOP: isize = 40;
pub const EPROTONOSUPPORT: isize = 93;
pub const EAFNOSUPPORT: isize = 97;
pub const EPROTOTYPE: isize = 91;
pub const ENOTSOCK: isize = 88;
pub const EDESTADDRREQ: isize = 89;
pub const EMSGSIZE: isize = 90;
pub const EOPNOTSUPP: isize = 95;
pub const ETIMEDOUT: isize = 110;
pub const ECONNREFUSED: isize = 111;
pub const EINPROGRESS: isize = 115;

pub const AT_FDCWD: i32 = -100;
pub const AT_EMPTY_PATH: i32 = 0x1000;

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;
pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;

pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

pub const RLIMIT_STACK: i32 = 3;
pub const RLIMIT_NOFILE: i32 = 7;

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;

pub static mut UPTIME_NS: u64 = 0;
pub static mut BOOT_WALL_NS: u64 = 0;

fn now_ns() -> u64 {
    unsafe { UPTIME_NS }
}

fn tick() {
    let t = crate::arch::rdtime();
    unsafe {
        UPTIME_NS = t * 100; // 10 MHz timebase
    }
}

pub fn user_slice(ptr: usize, len: usize) -> Option<&'static mut [u8]> {
    if !mm::in_user_range(ptr, len) {
        return None;
    }
    unsafe { Some(core::slice::from_raw_parts_mut(ptr as *mut u8, len)) }
}

pub fn user_slice_ro(ptr: usize, len: usize) -> Option<&'static [u8]> {
    if !mm::in_user_range(ptr, len) {
        return None;
    }
    unsafe { Some(core::slice::from_raw_parts(ptr as *const u8, len)) }
}

pub fn cstr(ptr: usize) -> Option<Vec<u8>> {
    if ptr < mm::MAIN_BASE || ptr >= mm::USER_TOP {
        return None;
    }
    let mut v = Vec::new();
    unsafe {
        let mut p = ptr as *const u8;
        loop {
            let c = *p;
            if c == 0 {
                break;
            }
            v.push(c);
            p = p.add(1);
            if v.len() > 4096 {
                return None;
            }
        }
    }
    Some(v)
}

fn read_u64(ptr: usize) -> u64 {
    unsafe { core::ptr::read_unaligned(ptr as *const u64) }
}
fn write_u64(ptr: usize, v: u64) {
    unsafe { core::ptr::write_unaligned(ptr as *mut u64, v) };
}
fn write_u32(ptr: usize, v: u32) {
    unsafe { core::ptr::write_unaligned(ptr as *mut u32, v) };
}
fn write_i32(ptr: usize, v: i32) {
    unsafe { core::ptr::write_unaligned(ptr as *mut i32, v) };
}
fn write_i64(ptr: usize, v: i64) {
    unsafe { core::ptr::write_unaligned(ptr as *mut i64, v) };
}

fn write_timespec(ptr: usize, sec: i64, nsec: i64) {
    if mm::in_user_range(ptr, 16) {
        write_i64(ptr, sec);
        write_i64(ptr + 8, nsec);
    }
}

fn do_mmap(addr: usize, len: usize, _prot: u32, flags: u32, fd: i32, off: u64) -> isize {
    if len == 0 {
        return -EINVAL;
    }
    let len_p = mm::align_up(len, 4096);
    let fixed = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    let target = if fixed {
        if addr & 0xfff != 0 {
            return -EINVAL;
        }
        addr
    } else {
        match mm::mmap_alloc(len_p) {
            Some(a) => a,
            None => return -ENOMEM,
        }
    };
    if target == 0 || target >= mm::USER_TOP || target + len_p > mm::USER_TOP {
        return -ENOMEM;
    }
    unsafe {
        core::ptr::write_bytes(target as *mut u8, 0, len_p);
    }
    if flags & MAP_ANONYMOUS == 0 {
        let f = match fs::file(fd as usize) {
            Some(f) => f,
            None => return -EBADF,
        };
        if f.kind != fs::FdKind::File {
            return -ENODEV;
        }
        let node = f.node;
        if fs::get(node).mode & fs::S_IFMT != fs::S_IFREG {
            return -ENODEV;
        }
        let size = fs::node_size(node) as u64;
        if off < size {
            let avail = (size - off) as usize;
            let n = avail.min(len_p);
            let data = fs::node_data(node);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(off as usize),
                    target as *mut u8,
                    n,
                );
            }
        }
    }
    target as isize
}

fn do_uname(ptr: usize) -> isize {
    if !mm::in_user_range(ptr, 390) {
        return -EFAULT;
    }
    unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, 390);
    }
    let fields: [&[u8]; 6] = [
        b"Linux",
        b"jiege",
        b"6.1.0",
        b"#1 SMP jiege-os",
        b"riscv64",
        b"(none)",
    ];
    for (i, f) in fields.iter().enumerate() {
        let base = ptr + i * 65;
        unsafe {
            core::ptr::copy_nonoverlapping(f.as_ptr(), base as *mut u8, f.len());
        }
    }
    0
}

fn do_sysinfo(ptr: usize) -> isize {
    if !mm::in_user_range(ptr, 112) {
        return -EFAULT;
    }
    unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, 112);
    }
    write_i64(ptr, (now_ns() / 1_000_000_000) as i64);
    write_u64(ptr + 8, 0);
    write_u64(ptr + 16, 0);
    write_u64(ptr + 24, 0);
    write_u64(ptr + 32, 100 * 1024 * 1024);
    write_u64(ptr + 40, 60 * 1024 * 1024);
    write_u64(ptr + 48, 0);
    write_u64(ptr + 56, 0);
    write_u64(ptr + 64, 0);
    write_u64(ptr + 72, 0);
    write_u32(ptr + 80, 1);
    write_u32(ptr + 84, 0);
    write_u64(ptr + 88, 0);
    write_u64(ptr + 96, 0);
    write_u32(ptr + 104, 1);
    0
}

fn do_getrlimit(res: i32, ptr: usize) -> isize {
    if ptr == 0 || !mm::in_user_range(ptr, 16) {
        return -EFAULT;
    }
    let (cur, max) = match res {
        RLIMIT_STACK => (8 * 1024 * 1024u64, 8 * 1024 * 1024u64),
        RLIMIT_NOFILE => (1024u64, 1024u64),
        0 => (u64::MAX, u64::MAX),
        _ => (u64::MAX, u64::MAX),
    };
    write_u64(ptr, cur);
    write_u64(ptr + 8, max);
    0
}

fn do_rt_sigaction(_sig: i32, act: usize, old: usize) -> isize {
    if old != 0 && mm::in_user_range(old, 32) {
        unsafe {
            core::ptr::write_bytes(old as *mut u8, 0, 32);
        }
    }
    let _ = act;
    0
}

fn do_exit(code: i32) -> ! {
    println!("[exit] process exited with code {}", code);
    crate::sbi::shutdown(code != 0);
}

pub fn dispatch(tf: &mut TrapFrame) {
    tick();
    let nr = tf.x[17];
    let a0 = tf.x[10];
    let a1 = tf.x[11];
    let a2 = tf.x[12];
    let a3 = tf.x[13];
    let a4 = tf.x[14];
    let a5 = tf.x[15];
    let r = match nr {
        READ => {
            let fd = a0;
            match user_slice(a1, a2) {
                Some(buf) => fs::read_fd(fd, buf),
                None => -EFAULT,
            }
        }
        WRITE => {
            let fd = a0;
            match user_slice_ro(a1, a2) {
                Some(buf) => fs::write_fd(fd, buf),
                None => -EFAULT,
            }
        }
        WRITEV | READV => {
            let fd = a0 as usize;
            let iov = a1;
            let cnt = a2;
            let mut total: isize = 0;
            for i in 0..cnt.min(64) {
                let base = read_u64(iov + i * 16) as usize;
                let len = read_u64(iov + i * 16 + 8) as usize;
                let r = if nr == WRITEV {
                    match user_slice_ro(base, len) {
                        Some(b) => fs::write_fd(fd, b),
                        None => -EFAULT,
                    }
                } else {
                    match user_slice(base, len) {
                        Some(b) => fs::read_fd(fd, b),
                        None => -EFAULT,
                    }
                };
                if r < 0 {
                    if total == 0 {
                        total = r;
                    }
                    break;
                }
                total += r;
                if (r as usize) < len {
                    break;
                }
            }
            total
        }
        PREAD64 => {
            let fd = a0;
            let off = a3;
            let r = fs::lseek(fd, off as i64, 0);
            if r < 0 {
                r
            } else {
                match user_slice(a1, a2) {
                    Some(buf) => fs::read_fd(fd, buf),
                    None => -EFAULT,
                }
            }
        }
        OPENAT => {
            let path = match cstr(a1) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            crate::trace_syscall(nr, &path);
            match fs::open(&path, a2 as u32, a3 as u32) {
                Ok(fd) => fd as isize,
                Err(e) => -(e as isize),
            }
        }
        CLOSE => fs::close(a0) as isize,
        LSEEK => fs::lseek(a0, a1 as i64, a2 as i32) as isize,
        NEWFSTATAT => {
            let flags = a3 as i32;
            if flags & AT_EMPTY_PATH != 0 && a1 == 0 {
                fs::fstat(a0, a2 as *mut u8) as isize
            } else {
                let path = match cstr(a1) {
                    Some(p) => p,
                    None => return tf.set_ret(-EFAULT),
                };
                let follow = flags & 0x100 == 0; // AT_SYMLINK_NOFOLLOW
                fs::stat_path(&path, follow, a2 as *mut u8) as isize
            }
        }
        FSTAT => fs::fstat(a0, a1 as *mut u8) as isize,
        STATX => -ENOSYS,
        GETDENTS64 => match user_slice(a1, a2) {
            Some(buf) => fs::getdents(a0, buf),
            None => -EFAULT,
        },
        READLINKAT => {
            let path = match cstr(a1) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            match user_slice(a2, a3) {
                Some(buf) => fs::readlink(&path, buf),
                None => -EFAULT,
            }
        }
        FACESSAT | FACCESSAT2 => {
            let path = match cstr(a1) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            fs::access(&path, a2 as i32) as isize
        }
        GETCWD => {
            let cwd = unsafe { fs::cwd().clone() };
            if a2 as usize <= cwd.len() {
                return tf.set_ret(-ERANGE);
            }
            if let Some(buf) = user_slice(a0, cwd.len() + 1) {
                buf[..cwd.len()].copy_from_slice(&cwd);
                buf[cwd.len()] = 0;
                cwd.len() as isize + 1
            } else {
                -EFAULT
            }
        }
        CHDIR => {
            let path = match cstr(a0) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            match fs::lookup(&path, true) {
                Some(i) if fs::get(i).mode & fs::S_IFMT == fs::S_IFDIR => {
                    unsafe {
                        *fs::cwd() = fs::norm(&path);
                    }
                    0
                }
                Some(_) => -ENOTDIR,
                None => -ENOENT,
            }
        }
        MKDIRAT => {
            let path = match cstr(a1) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            let p = fs::norm(&path);
            if fs::lookup(&p, true).is_some() {
                -EEXIST
            } else {
                fs::add_node(p, fs::S_IFDIR | (a2 as u32 & 0o7777), fs::DataRef::Static(&[]), None);
                0
            }
        }
        UNLINKAT => {
            let path = match cstr(a1) {
                Some(p) => p,
                None => return tf.set_ret(-EFAULT),
            };
            match fs::lookup(&path, false) {
                Some(i) => {
                    fs::get_mut(i).deleted = true;
                    0
                }
                None => -ENOENT,
            }
        }
        FSYNC | FDATASYNC | SYNC => 0,
        FTRUNCATE => {
            if a0 < 1024 {
                if let Some(f) = fs::file(a0) {
                    if f.kind == fs::FdKind::File {
                        fs::truncate_node(f.node);
                        let node = f.node;
                        let _ = node;
                        return tf.set_ret(0);
                    }
                }
            }
            -EBADF
        }
        MMAP => do_mmap(a0, a1, a2 as u32, a3 as u32, a4 as i32, a5 as u64),
        MUNMAP => {
            if a0 & 0xfff != 0 {
                -EINVAL
            } else {
                mm::mmap_free(a0, a1);
                0
            }
        }
        MREMAP => {
            let old = a0;
            let oldsz = a1;
            let newsz = a2;
            if newsz <= oldsz {
                old as isize
            } else {
                let r = do_mmap(0, newsz, 0, MAP_ANONYMOUS | MAP_PRIVATE, -1, 0);
                if r < 0 {
                    return tf.set_ret(r);
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(old as *const u8, r as *mut u8, oldsz);
                }
                mm::mmap_free(old, oldsz);
                r
            }
        }
        MPROTECT | MADVISE | MSYNC | MLOCK | MUNLOCK => 0,
        BRK => {
            let addr = a0;
            unsafe {
                if addr == 0 {
                    return tf.set_ret(crate::user::BRK_CUR as isize);
                }
                if addr < crate::user::BRK_START || addr > mm::MAIN_LIMIT {
                    return tf.set_ret(crate::user::BRK_CUR as isize);
                }
                crate::user::BRK_CUR = addr;
                crate::user::BRK_CUR as isize
            }
        }
        SET_TID_ADDRESS => 1,
        SET_ROBUST_LIST => 0,
        RSEQ => -ENOSYS,
        FUTEX => match a1 as i32 {
            0 => {
                // FUTEX_WAIT: single-threaded, fail with EAGAIN if value matches
                let uaddr = a0 as *const u32;
                if mm::in_user_range(a0, 4) && unsafe { *uaddr } == a2 as u32 {
                    0
                } else {
                    -EAGAIN
                }
            }
            _ => 0,
        },
        GETPID | GETTID => 1,
        GETPPID => 0,
        GETUID | GETEUID | GETGID | GETEGID => 0,
        GETRESUID => {
            if mm::in_user_range(a0, 12) {
                write_u32(a0, 0);
                write_u32(a0 + 4, 0);
                write_u32(a0 + 8, 0);
                0
            } else {
                -EFAULT
            }
        }
        GETRESGID => {
            if mm::in_user_range(a0, 12) {
                write_u32(a0, 0);
                write_u32(a0 + 4, 0);
                write_u32(a0 + 8, 0);
                0
            } else {
                -EFAULT
            }
        }
        SETUID | SETGID | SETREUID | SETREGID | SETRESUID | SETRESGID | SETGROUPS | SETFSGID
        | SETFSUID => 0,
        GETGROUPS => 0,
        UNAME => do_uname(a0),
        SYSINFO => do_sysinfo(a0),
        GETRLIMIT => do_getrlimit(a0 as i32, a1),
        PRLIMIT64 => {
            let res = a1 as i32;
            if a3 != 0 {
                do_getrlimit(res, a3)
            } else {
                0
            }
        }
        GETRUSAGE => {
            if mm::in_user_range(a0, 144) {
                unsafe {
                    core::ptr::write_bytes(a0 as *mut u8, 0, 144);
                }
                0
            } else {
                -EFAULT
            }
        }
        TIMES => {
            if a0 != 0 && mm::in_user_range(a0, 32) {
                unsafe {
                    core::ptr::write_bytes(a0 as *mut u8, 0, 32);
                }
            }
            (now_ns() / 10_000_000) as isize
        }
        CLOCK_GETTIME => {
            let clk = a0 as i32;
            let ns = match clk {
                CLOCK_REALTIME => unsafe { BOOT_WALL_NS + now_ns() },
                _ => now_ns(),
            };
            write_timespec(a1, (ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64);
            0
        }
        CLOCK_GETRES => {
            write_timespec(a1, 0, 1_000_000);
            0
        }
        GETTIMEOFDAY => {
            let ns = unsafe { BOOT_WALL_NS + now_ns() };
            write_i64(a0, (ns / 1_000_000_000) as i64);
            write_i64(a0 + 8, (ns % 1_000_000_000) as i64 / 1000);
            0
        }
        NANOSLEEP | CLOCK_NANOSLEEP => {
            let req = if nr == NANOSLEEP { a0 } else { a2 };
            if !mm::in_user_range(req, 16) {
                return tf.set_ret(-EFAULT);
            }
            let sec = read_u64(req) as i64;
            let nsec = read_u64(req + 8) as i64;
            let dur = (sec as u64) * 1_000_000_000 + nsec as u64;
            let deadline = now_ns() + dur;
            while now_ns() < deadline {
                net::poll();
                tick();
            }
            0
        }
        RT_SIGACTION => do_rt_sigaction(a0 as i32, a1, a2),
        RT_SIGPROCMASK => 0,
        SIGALTSTACK => 0,
        RT_SIGRETURN => -ENOSYS,
        KILL | TKILL | TGKILL => 0,
        PRCTL => 0,
        IOPRIO_SET | IOPRIO_GET => 0,
        SCHED_YIELD => 0,
        SCHED_GETAFFINITY => {
            if mm::in_user_range(a2, 8) {
                write_u64(a2, 1);
                8
            } else {
                -EFAULT
            }
        }
        SCHED_SETAFFINITY => 0,
        SCHED_GET_PRIORITY_MAX | SCHED_GET_PRIORITY_MIN => 0,
        GETPRIORITY | SETPRIORITY => 0,
        UMOUNT2 | MOUNT => -EPERM,
        REBOOT => -EPERM,
        SETPGID | GETPGID | SETSID | GETSID => 1,
        UMASK => 0o022,
        FCNTL => {
            let fd = a0 as usize;
            let cmd = a1 as i32;
            let arg = a2;
            match cmd {
                0 | 1030 => {
                    // F_DUPFD, F_DUPFD_CLOEXEC
                    let f = match fs::file(fd) {
                        Some(f) => f.clone(),
                        None => return tf.set_ret(-EBADF),
                    };
                    let nfd = fs::alloc_fd(f);
                    if nfd == usize::MAX {
                        -EMFILE
                    } else {
                        nfd as isize
                    }
                }
                1 | 2 => 0, // F_GETFD / F_SETFD
                3 => match fs::file(fd) {
                    Some(f) => f.flags as isize & !(fs::O_CLOEXEC),
                    None => -EBADF,
                },
                4 => {
                    if let Some(f) = fs::file(fd) {
                        f.flags = (f.flags & !(fs::O_NONBLOCK)) | (arg as u32 & fs::O_NONBLOCK);
                        0
                    } else {
                        -EBADF
                    }
                }
                _ => 0,
            }
        }
        IOCTL => {
            let fd = a0 as usize;
            let req = a1 as u32;
            match req {
                0x5421 => {
                    // FIONBIO
                    if let Some(f) = fs::file(fd) {
                        let on = read_u64(a2) as u32 != 0;
                        if on {
                            f.flags |= fs::O_NONBLOCK;
                        } else {
                            f.flags &= !fs::O_NONBLOCK;
                        }
                        0
                    } else {
                        -EBADF
                    }
                }
                0x541B => {
                    // FIONREAD
                    if let Some(f) = fs::file(fd) {
                        let n = match f.kind {
                            fs::FdKind::Socket(id) => net::pending(id),
                            _ => 0,
                        };
                        write_i32(a2, n as i32);
                        0
                    } else {
                        -EBADF
                    }
                }
                0x5401 => 0, // TCGETS
                _ => -ENOTTY,
            }
        }
        DUP => {
            let f = match fs::file(a0 as usize) {
                Some(f) => f.clone(),
                None => return tf.set_ret(-EBADF),
            };
            let nfd = fs::alloc_fd(f);
            if nfd == usize::MAX {
                -EMFILE
            } else {
                nfd as isize
            }
        }
        DUP3 => {
            let old = a0 as usize;
            let new = a1 as usize;
            if old == new {
                return tf.set_ret(-EINVAL);
            }
            let f = match fs::file(old) {
                Some(f) => f.clone(),
                None => return tf.set_ret(-EBADF),
            };
            if new >= 1024 {
                return tf.set_ret(-EBADF);
            }
            unsafe {
                fs::files()[new] = Some(f);
            }
            new as isize
        }
        PIPE2 => match user_slice(a0, 8) {
            Some(buf) => net::pipe2(buf, a1 as u32),
            None => -EFAULT,
        },
        SOCKET => net::socket(a0 as i32, a1 as i32, a2 as i32),
        BIND => {
            let addr = match user_slice_ro(a1, a2) {
                Some(b) => b,
                None => return tf.set_ret(-EFAULT),
            };
            net::bind(a0 as u32, addr)
        }
        LISTEN => net::listen(a0 as u32, a1 as i32),
        ACCEPT | ACCEPT4 => {
            let flags = if nr == ACCEPT4 { a3 as u32 } else { 0 };
            let mut addr = [0u8; 128];
            let mut res = net::accept(a0 as u32, flags, &mut addr);
            if res >= 0 {
                if a1 != 0 && a2 != 0 {
                    if let Some(buf) = user_slice(a1, 16) {
                        buf.copy_from_slice(&addr[..16]);
                    }
                    write_u32(a2, 16);
                }
            } else if res == -EAGAIN {
                // try again after polling
                net::poll();
                res = net::accept(a0 as u32, flags, &mut addr);
                if res >= 0 && a1 != 0 {
                    if let Some(buf) = user_slice(a1, 16) {
                        buf.copy_from_slice(&addr[..16]);
                    }
                    if a2 != 0 {
                        write_u32(a2, 16);
                    }
                }
            }
            res
        }
        CONNECT => {
            let addr = match user_slice_ro(a1, a2) {
                Some(b) => b,
                None => return tf.set_ret(-EFAULT),
            };
            net::connect(a0 as u32, addr)
        }
        GETSOCKNAME => match user_slice(a1, a2) {
            Some(buf) => net::getsockname(a0 as u32, buf),
            None => -EFAULT,
        },
        GETPEERNAME => match user_slice(a1, a2) {
            Some(buf) => net::getpeername(a0 as u32, buf),
            None => -EFAULT,
        },
        SENDFILE => {
            let out = a0 as usize;
            let infd = a1 as usize;
            let offp = a2;
            let count = a3 as usize;
            let f = match fs::file(infd) {
                Some(f) => f.clone(),
                None => return tf.set_ret(-EBADF),
            };
            if f.kind != fs::FdKind::File {
                return tf.set_ret(-EINVAL);
            }
            let size = fs::node_size(f.node);
            let mut off = if offp != 0 { read_u64(offp) } else { f.pos };
            if off >= size {
                return tf.set_ret(0);
            }
            let n = count.min((size - off) as usize).min(1 << 20);
            let data = fs::node_data(f.node);
            let r = net::send(out as u32, &data[off as usize..off as usize + n]);
            if r > 0 {
                off += r as u64;
                if offp != 0 {
                    write_u64(offp, off);
                } else if let Some(f) = fs::file(infd) {
                    f.pos = off;
                }
            }
            r
        }
        RECVFROM | RECVMSG => {
            let fd = a0 as usize;
            let is_msg = nr == RECVMSG;
            let (bufp, len) = if is_msg {
                let iov = read_u64(a1 + 16) as usize; // msg_iov
                (iov, read_u64(a1 + 24) as usize) // wrong-ish; musl uses iov[0]
            } else {
                (a1, a2)
            };
            let f = match fs::file(fd) {
                Some(f) => f.clone(),
                None => return tf.set_ret(-EBADF),
            };
            if let fs::FdKind::Socket(id) = f.kind {
                if is_msg {
                    // msg_iov is a pointer to iovec
                    let iov = read_u64(a1 + 16) as usize;
                    let base = read_u64(iov) as usize;
                    let l = read_u64(iov + 8) as usize;
                    match user_slice(base, l) {
                        Some(b) => net::recv(id, b),
                        None => -EFAULT,
                    }
                } else {
                    match user_slice(bufp, len) {
                        Some(b) => net::recvfrom(id, b, &mut []),
                        None => -EFAULT,
                    }
                }
            } else {
                match user_slice(bufp, len) {
                    Some(b) => fs::read_fd(fd, b),
                    None => -EFAULT,
                }
            }
        }
        SENDTO | SENDMSG => {
            let fd = a0 as usize;
            let is_msg = nr == SENDMSG;
            let f = match fs::file(fd) {
                Some(f) => f.clone(),
                None => return tf.set_ret(-EBADF),
            };
            if let fs::FdKind::Socket(id) = f.kind {
                if is_msg {
                    let iov = read_u64(a1 + 16) as usize;
                    let base = read_u64(iov) as usize;
                    let l = read_u64(iov + 8) as usize;
                    match user_slice_ro(base, l) {
                        Some(b) => net::send(id, b),
                        None => -EFAULT,
                    }
                } else {
                    match user_slice_ro(a1, a2) {
                        Some(b) => net::sendto(id, b, &[]),
                        None => -EFAULT,
                    }
                }
            } else {
                match user_slice_ro(a1, a2) {
                    Some(b) => fs::write_fd(fd, b),
                    None => -EFAULT,
                }
            }
        }
        SETSOCKOPT => match user_slice_ro(a3, a4) {
            Some(v) => net::setsockopt(a0 as u32, a1 as i32, a2 as i32, v),
            None => -EFAULT,
        },
        GETSOCKOPT => match user_slice(a3, a4) {
            Some(v) => net::getsockopt(a0 as u32, a1 as i32, a2 as i32, v),
            None => -EFAULT,
        },
        SHUTDOWN => net::shutdown_socket(a0 as u32, a1 as i32),
        EPOLL_CREATE1 => net::epoll_create(),
        EPOLL_CTL => match user_slice_ro(a3, 12) {
            Some(ev) => net::epoll_ctl(a0 as u32, a1 as i32, a2 as i32, ev),
            None => -EFAULT,
        },
        EPOLL_PWAIT | EPOLL_PWAIT2 => {
            let maxevents = a2 as i32;
            let timeout = if nr == EPOLL_PWAIT { a4 as i32 } else { -1 };
            match user_slice(a1, (maxevents as usize) * 12) {
                Some(ev) => net::epoll_wait(a0 as u32, ev, maxevents, timeout),
                None => -EFAULT,
            }
        }
        GETRANDOM => {
            if let Some(buf) = user_slice(a0, a1) {
                let mut x = unsafe { crate::arch::rdtime() } ^ 0x9e3779b97f4a7c15;
                for b in buf.iter_mut() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *b = x as u8;
                }
                a1 as isize
            } else {
                -EFAULT
            }
        }
        RISCV_HWPROBE => {
            let pairs = a0;
            let count = a1;
            if mm::in_user_range(pairs, count * 16) {
                for i in 0..count {
                    let key = read_u64(pairs + i * 16);
                    let val = match key {
                        4 => 1u64, // TIME_CSR_IMPLEMENTED
                        5 => 0u64, // IMA_EXT_0: no bitmanip
                        _ => 0u64,
                    };
                    write_u64(pairs + i * 16 + 8, val);
                }
                0
            } else {
                -EFAULT
            }
        }
        RISCV_FLUSH_ICACHE => {
            crate::arch::fence_i();
            0
        }
        EXIT => do_exit(a0 as i32),
        EXIT_GROUP => do_exit(a0 as i32),
        WAIT4 | WAITID => -ECHILD,
        CLONE | CLONE3 => -ENOSYS,
        EXECVE => -ENOSYS,
        MEMBARRIER => 0,
        GETCPU => 0,
        SETITIMER | GETITIMER => 0,
        TIMERFD_CREATE | TIMERFD_SETTIME | TIMERFD_GETTIME => -ENOSYS,
        EVENTFD2 => -ENOSYS,
        SIGNALFD4 => -ENOSYS,
        FALLOCATE => 0,
        FLOCK => 0,
        STATFS | FSTATFS => {
            if mm::in_user_range(if nr == STATFS { a1 } else { a1 }, 64) {
                unsafe {
                    core::ptr::write_bytes(a1 as *mut u8, 0, 64);
                }
                0
            } else {
                -EFAULT
            }
        }
        _ => {
            println!("[syscall] unimplemented nr={} args={:#x},{:#x},{:#x}", nr, a0, a1, a2);
            -ENOSYS
        }
    };
    tf.set_ret(r);
}
