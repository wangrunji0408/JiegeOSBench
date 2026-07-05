//! Linux riscv64 syscall ABI emulation.
pub mod fs;
pub mod mm;
pub mod net;
pub mod proc;
pub mod time;

use crate::task::current;
use alloc::string::String;

pub type SysResult = Result<usize, i32>;

// errno values
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const EINTR: i32 = 4;
pub const EBADF: i32 = 9;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EEXIST: i32 = 17;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENOTTY: i32 = 25;
pub const ESPIPE: i32 = 29;
pub const EPIPE: i32 = 32;
pub const ERANGE: i32 = 34;
pub const ENOSYS: i32 = 38;
pub const ENOTSOCK: i32 = 88;
pub const EOPNOTSUPP: i32 = 95;
pub const EADDRINUSE: i32 = 98;
pub const ENETUNREACH: i32 = 101;
pub const ECONNRESET: i32 = 104;
pub const ENOTCONN: i32 = 107;
pub const ECONNREFUSED: i32 = 111;
pub const EINPROGRESS: i32 = 115;

// ---------- user memory helpers ----------
// The user page table is always active and SUM=1, so kernel code can access
// user memory directly. We validate mappings to fail with EFAULT rather than
// take a kernel page fault.

pub fn check_user_range(va: usize, len: usize) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    let t = current();
    let start = crate::mm::page_down(va);
    let end = crate::mm::page_up(va + len);
    let mut p = start;
    while p < end {
        if t.pt.translate(p).is_none() {
            return Err(EFAULT);
        }
        p += crate::mm::PAGE_SIZE;
    }
    Ok(())
}

pub fn user_slice(va: usize, len: usize) -> Result<&'static [u8], i32> {
    check_user_range(va, len)?;
    Ok(unsafe { core::slice::from_raw_parts(va as *const u8, len) })
}

pub fn user_slice_mut(va: usize, len: usize) -> Result<&'static mut [u8], i32> {
    check_user_range(va, len)?;
    Ok(unsafe { core::slice::from_raw_parts_mut(va as *mut u8, len) })
}

pub fn read_user<T: Copy>(va: usize) -> Result<T, i32> {
    check_user_range(va, core::mem::size_of::<T>())?;
    Ok(unsafe { core::ptr::read_unaligned(va as *const T) })
}

pub fn write_user<T: Copy>(va: usize, v: T) -> Result<(), i32> {
    check_user_range(va, core::mem::size_of::<T>())?;
    unsafe { core::ptr::write_unaligned(va as *mut T, v) };
    Ok(())
}

pub fn read_cstr(va: usize) -> Result<String, i32> {
    let mut s = String::new();
    let mut p = va;
    loop {
        let b: u8 = read_user(p)?;
        if b == 0 {
            return Ok(s);
        }
        s.push(b as char);
        p += 1;
        if s.len() > 4096 {
            return Err(EINVAL);
        }
    }
}

/// Main syscall dispatcher. Returns the value to place in a0.
pub fn dispatch(nr: usize, a: [usize; 6]) -> usize {
    let r = do_syscall(nr, a);
    let ret = match r {
        Ok(v) => v,
        Err(e) => (-e as isize) as usize,
    };
    if crate::DEBUG_SYSCALLS {
        println!(
            "[sys] {} ({:#x},{:#x},{:#x},{:#x}) = {}",
            sysname(nr),
            a[0],
            a[1],
            a[2],
            a[3],
            ret as isize
        );
    }
    ret
}

fn do_syscall(nr: usize, a: [usize; 6]) -> SysResult {
    match nr {
        17 => fs::getcwd(a[0], a[1]),
        19 => fs::eventfd2(a[0], a[1]),
        20 => net::epoll_create1(a[0]),
        21 => net::epoll_ctl(a[0], a[1], a[2], a[3]),
        22 => net::epoll_pwait(a[0], a[1], a[2], a[3] as isize),
        23 => fs::dup(a[0]),
        24 => fs::dup3(a[0], a[1], a[2]),
        25 => fs::fcntl(a[0], a[1], a[2]),
        29 => fs::ioctl(a[0], a[1], a[2]),
        34 => fs::mkdirat(a[0], a[1], a[2]),
        35 => fs::unlinkat(a[0], a[1], a[2]),
        46 => fs::ftruncate(a[0], a[1]),
        48 | 439 => fs::faccessat(a[0], a[1], a[2]),
        49 => fs::chdir(a[0]),
        56 => fs::openat(a[0] as isize, a[1], a[2], a[3]),
        57 => fs::close(a[0]),
        59 => fs::pipe2(a[0], a[1]),
        61 => fs::getdents64(a[0], a[1], a[2]),
        62 => fs::lseek(a[0], a[1] as isize, a[2]),
        63 => fs::read(a[0], a[1], a[2]),
        64 => fs::write(a[0], a[1], a[2]),
        65 => fs::readv(a[0], a[1], a[2]),
        66 => fs::writev(a[0], a[1], a[2]),
        67 => fs::pread64(a[0], a[1], a[2], a[3]),
        68 => fs::pwrite64(a[0], a[1], a[2], a[3]),
        71 => fs::sendfile(a[0], a[1], a[2], a[3]),
        73 => net::ppoll(a[0], a[1], a[2]),
        78 => fs::readlinkat(a[0], a[1], a[2], a[3]),
        79 => fs::fstatat(a[0] as isize, a[1], a[2], a[3]),
        80 => fs::fstat(a[0], a[1]),
        81 | 82 | 83 => Ok(0), // sync/fsync/fdatasync
        88 => Ok(0),           // utimensat
        93 | 94 => proc::exit(a[0] as i32),
        96 => proc::set_tid_address(a[0]),
        98 => proc::futex(a[0], a[1], a[2], a[3]),
        99 => Ok(0), // set_robust_list
        101 => time::nanosleep(a[0], a[1]),
        102 => Ok(0),  // getitimer
        103 => Ok(0),  // setitimer (nginx uses timer_resolution only if set)
        113 => time::clock_gettime(a[0], a[1]),
        114 => time::clock_getres(a[0], a[1]),
        115 => time::clock_nanosleep(a[0], a[1], a[2], a[3]),
        122 => proc::sched_getaffinity(a[0], a[1], a[2]),
        124 => Ok(0), // sched_yield
        129 | 130 | 131 => Ok(0), // kill/tkill/tgkill (no other procs)
        132 => Ok(0), // sigaltstack
        134 => proc::rt_sigaction(a[0], a[1], a[2]),
        135 => proc::rt_sigprocmask(a[0], a[1], a[2]),
        137 => Err(EAGAIN), // rt_sigtimedwait
        139 => Ok(0),       // rt_sigreturn (we never deliver signals)
        144..=153 => Ok(0), // set*id
        154 | 155 => Ok(0), // setpgid/getpgid
        157 => Ok(0),       // setsid
        158 => Ok(0),       // getgroups
        159 => Ok(0),       // setgroups
        160 => proc::uname(a[0]),
        163 => proc::getrlimit(a[0], a[1]),
        164 => Ok(0), // setrlimit
        165 => proc::getrusage(a[0], a[1]),
        166 => Ok(0o22), // umask
        167 => Ok(0),    // prctl
        169 => time::gettimeofday(a[0], a[1]),
        172 => Ok(1),  // getpid
        173 => Ok(0),  // getppid
        174..=177 => Ok(0), // get[e]uid/gid → root
        178 => Ok(1),  // gettid
        179 => proc::sysinfo(a[0]),
        198 => net::socket(a[0], a[1], a[2]),
        199 => net::socketpair(a[0], a[1], a[2], a[3]),
        200 => net::bind(a[0], a[1], a[2]),
        201 => net::listen(a[0], a[1]),
        202 => net::accept4(a[0], a[1], a[2], 0),
        203 => net::connect(a[0], a[1], a[2]),
        204 => net::getsockname(a[0], a[1], a[2]),
        205 => net::getpeername(a[0], a[1], a[2]),
        206 => net::sendto(a[0], a[1], a[2], a[3], a[4], a[5]),
        207 => net::recvfrom(a[0], a[1], a[2], a[3], a[4], a[5]),
        208 => net::setsockopt(a[0], a[1], a[2], a[3], a[4]),
        209 => net::getsockopt(a[0], a[1], a[2], a[3], a[4]),
        210 => net::shutdown(a[0], a[1]),
        211 => net::sendmsg(a[0], a[1], a[2]),
        212 => net::recvmsg(a[0], a[1], a[2]),
        214 => mm::brk(a[0]),
        215 => mm::munmap(a[0], a[1]),
        216 => mm::mremap(a[0], a[1], a[2], a[3], a[4]),
        220 => Err(ENOSYS), // clone
        221 => Err(ENOSYS), // execve
        222 => mm::mmap(a[0], a[1], a[2], a[3], a[4] as isize, a[5]),
        223 => Ok(0), // fadvise64
        226 => mm::mprotect(a[0], a[1], a[2]),
        227 => Ok(0), // msync
        233 => Ok(0), // madvise
        242 => net::accept4(a[0], a[1], a[2], a[3]),
        258 => Err(ENOSYS), // riscv_hwprobe
        259 => proc::riscv_flush_icache(),
        260 => Err(ENOSYS), // wait4 (no children)
        261 => proc::prlimit64(a[0], a[1], a[2], a[3]),
        278 => proc::getrandom(a[0], a[1], a[2]),
        291 => fs::statx(a[0] as isize, a[1], a[2], a[3], a[4]),
        435 => Err(ENOSYS), // clone3
        436 => fs::close_range(a[0], a[1], a[2]),
        _ => {
            println!("[sys] UNIMPLEMENTED syscall {} args={:x?}", nr, a);
            Err(ENOSYS)
        }
    }
}

fn sysname(nr: usize) -> &'static str {
    match nr {
        17 => "getcwd", 19 => "eventfd2", 20 => "epoll_create1", 21 => "epoll_ctl",
        22 => "epoll_pwait", 23 => "dup", 24 => "dup3", 25 => "fcntl", 29 => "ioctl",
        34 => "mkdirat", 35 => "unlinkat", 46 => "ftruncate", 48 => "faccessat",
        49 => "chdir", 56 => "openat", 57 => "close", 59 => "pipe2", 61 => "getdents64",
        62 => "lseek", 63 => "read", 64 => "write", 65 => "readv", 66 => "writev",
        67 => "pread64", 68 => "pwrite64", 71 => "sendfile", 73 => "ppoll",
        78 => "readlinkat", 79 => "fstatat", 80 => "fstat", 93 => "exit",
        94 => "exit_group", 96 => "set_tid_address", 98 => "futex", 99 => "set_robust_list",
        101 => "nanosleep", 113 => "clock_gettime", 115 => "clock_nanosleep",
        122 => "sched_getaffinity", 134 => "rt_sigaction", 135 => "rt_sigprocmask",
        160 => "uname", 163 => "getrlimit", 169 => "gettimeofday", 172 => "getpid",
        174 => "getuid", 178 => "gettid", 179 => "sysinfo", 198 => "socket",
        199 => "socketpair", 200 => "bind", 201 => "listen", 202 => "accept",
        203 => "connect", 204 => "getsockname", 206 => "sendto", 207 => "recvfrom",
        208 => "setsockopt", 209 => "getsockopt", 210 => "shutdown", 211 => "sendmsg",
        212 => "recvmsg", 214 => "brk", 215 => "munmap", 216 => "mremap",
        220 => "clone", 222 => "mmap", 226 => "mprotect", 242 => "accept4",
        258 => "riscv_hwprobe", 259 => "riscv_flush_icache", 261 => "prlimit64",
        278 => "getrandom", 291 => "statx", 436 => "close_range",
        _ => "?",
    }
}
