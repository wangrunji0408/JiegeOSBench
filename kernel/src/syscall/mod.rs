//! Syscall dispatch (riscv64 Linux ABI) and user-memory helpers.

pub mod fs;
pub mod mem;
pub mod process;
pub mod signal;
pub mod socket;

use alloc::vec::Vec;

use crate::task::TrapFrame;

// riscv64 syscall numbers
pub const SYS_GETCWD: usize = 17;
pub const SYS_EVENTFD2: usize = 19;
pub const SYS_EPOLL_CREATE1: usize = 20;
pub const SYS_EPOLL_CTL: usize = 21;
pub const SYS_EPOLL_PWAIT: usize = 22;
pub const SYS_DUP: usize = 23;
pub const SYS_DUP3: usize = 24;
pub const SYS_FCNTL: usize = 25;
pub const SYS_IOCTL: usize = 29;
pub const SYS_MKDIRAT: usize = 34;
pub const SYS_UNLINKAT: usize = 35;
pub const SYS_RENAMEAT: usize = 38;
pub const SYS_STATFS: usize = 43;
pub const SYS_FSTATFS: usize = 44;
pub const SYS_TRUNCATE: usize = 45;
pub const SYS_FTRUNCATE: usize = 46;
pub const SYS_FACCESSAT: usize = 48;
pub const SYS_CHDIR: usize = 49;
pub const SYS_FCHDIR: usize = 50;
pub const SYS_FCHMOD: usize = 52;
pub const SYS_FCHMODAT: usize = 53;
pub const SYS_FCHOWNAT: usize = 54;
pub const SYS_FCHOWN: usize = 55;
pub const SYS_OPENAT: usize = 56;
pub const SYS_CLOSE: usize = 57;
pub const SYS_PIPE2: usize = 59;
pub const SYS_GETDENTS64: usize = 61;
pub const SYS_LSEEK: usize = 62;
pub const SYS_READ: usize = 63;
pub const SYS_WRITE: usize = 64;
pub const SYS_READV: usize = 65;
pub const SYS_WRITEV: usize = 66;
pub const SYS_PREAD64: usize = 67;
pub const SYS_PWRITE64: usize = 68;
pub const SYS_SENDFILE: usize = 71;
pub const SYS_READLINKAT: usize = 78;
pub const SYS_NEWFSTATAT: usize = 79;
pub const SYS_FSTAT: usize = 80;
pub const SYS_FSYNC: usize = 82;
pub const SYS_FDATASYNC: usize = 83;
pub const SYS_UTIMENSAT: usize = 88;
pub const SYS_EXIT: usize = 93;
pub const SYS_EXIT_GROUP: usize = 94;
pub const SYS_WAITID: usize = 95;
pub const SYS_SET_TID_ADDRESS: usize = 96;
pub const SYS_FUTEX: usize = 98;
pub const SYS_SET_ROBUST_LIST: usize = 99;
pub const SYS_GET_ROBUST_LIST: usize = 100;
pub const SYS_NANOSLEEP: usize = 101;
pub const SYS_CLOCK_GETTIME: usize = 113;
pub const SYS_CLOCK_GETRES: usize = 114;
pub const SYS_CLOCK_NANOSLEEP: usize = 115;
pub const SYS_SCHED_SETAFFINITY: usize = 122;
pub const SYS_SCHED_GETAFFINITY: usize = 123;
pub const SYS_SCHED_YIELD: usize = 124;
pub const SYS_KILL: usize = 129;
pub const SYS_TKILL: usize = 130;
pub const SYS_TGKILL: usize = 131;
pub const SYS_SIGALTSTACK: usize = 132;
pub const SYS_RT_SIGSUSPEND: usize = 133;
pub const SYS_RT_SIGACTION: usize = 134;
pub const SYS_RT_SIGPROCMASK: usize = 135;
pub const SYS_RT_SIGPENDING: usize = 136;
pub const SYS_RT_SIGTIMEDWAIT: usize = 137;
pub const SYS_RT_SIGRETURN: usize = 139;
pub const SYS_TIMES: usize = 153;
pub const SYS_SETPGID: usize = 154;
pub const SYS_GETPGID: usize = 155;
pub const SYS_SETSID: usize = 157;
pub const SYS_GETGROUPS: usize = 158;
pub const SYS_SETGROUPS: usize = 159;
pub const SYS_UNAME: usize = 160;
pub const SYS_SETHOSTNAME: usize = 161;
pub const SYS_GETRLIMIT: usize = 163;
pub const SYS_SETRLIMIT: usize = 164;
pub const SYS_GETRUSAGE: usize = 165;
pub const SYS_UMASK: usize = 166;
pub const SYS_PRCTL: usize = 167;
pub const SYS_PRlimit64: usize = 168;
pub const SYS_GETTIMEOFDAY: usize = 170;
pub const SYS_GETPID: usize = 172;
pub const SYS_GETPPID: usize = 173;
pub const SYS_GETUID: usize = 174;
pub const SYS_GETEUID: usize = 175;
pub const SYS_GETGID: usize = 176;
pub const SYS_GETEGID: usize = 177;
pub const SYS_GETTID: usize = 178;
pub const SYS_SYSINFO: usize = 179;
pub const SYS_SOCKET: usize = 198;
pub const SYS_SOCKETPAIR: usize = 199;
pub const SYS_BIND: usize = 200;
pub const SYS_LISTEN: usize = 201;
pub const SYS_ACCEPT: usize = 202;
pub const SYS_CONNECT: usize = 203;
pub const SYS_GETSOCKNAME: usize = 204;
pub const SYS_GETPEERNAME: usize = 205;
pub const SYS_SENDTO: usize = 206;
pub const SYS_RECVFROM: usize = 207;
pub const SYS_SETSOCKOPT: usize = 208;
pub const SYS_GETSOCKOPT: usize = 209;
pub const SYS_SHUTDOWN: usize = 210;
pub const SYS_SENDMSG: usize = 211;
pub const SYS_RECVMSG: usize = 212;
pub const SYS_BRK: usize = 214;
pub const SYS_MUNMAP: usize = 215;
pub const SYS_MREMAP: usize = 216;
pub const SYS_CLONE: usize = 220;
pub const SYS_EXECVE: usize = 221;
pub const SYS_MMAP: usize = 222;
pub const SYS_MPROTECT: usize = 226;
pub const SYS_MSYNC: usize = 227;
pub const SYS_MLOCK: usize = 228;
pub const SYS_MUNLOCK: usize = 229;
pub const SYS_MLOCKALL: usize = 230;
pub const SYS_MUNLOCKALL: usize = 231;
pub const SYS_MADVISE: usize = 233;
pub const SYS_ACCEPT4: usize = 242;
pub const SYS_WAIT4: usize = 244;
pub const SYS_RENAMEAT2: usize = 260;
pub const SYS_GETRANDOM: usize = 262;
pub const SYS_EXECVEAT: usize = 265;
pub const SYS_STATX: usize = 275;
pub const SYS_GETCPU: usize = 168 + 1; // placeholder (not used)
pub const SYS_RSEQ: usize = 277;
pub const SYS_CLONE3: usize = 290;
pub const SYS_CLOSE_RANGE: usize = 291;
pub const SYS_OPENAT2: usize = 292;

pub fn handle(tf: *mut TrapFrame) {
    let num = unsafe { (*tf).a7() };
    let a0 = unsafe { (*tf).a0() };
    let a1 = unsafe { (*tf).a1() };
    let a2 = unsafe { (*tf).a2() };
    let a3 = unsafe { (*tf).a3() };
    let a4 = unsafe { (*tf).a4() };
    let a5 = unsafe { (*tf).a5() };

    let ret: isize = dispatch(num, a0, a1, a2, a3, a4, a5);
    unsafe {
        (*tf).set_a0(ret as usize);
    }
}

fn dispatch(
    num: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> isize {
    match num {
        SYS_READ => fs::sys_read(a0, a1, a2),
        SYS_WRITE => fs::sys_write(a0, a1, a2),
        SYS_READV => fs::sys_readv(a0, a1, a2),
        SYS_WRITEV => fs::sys_writev(a0, a1, a2),
        SYS_PREAD64 => fs::sys_pread64(a0, a1, a2, a3),
        SYS_PWRITE64 => fs::sys_pwrite64(a0, a1, a2, a3),
        SYS_OPENAT => fs::sys_openat(a0 as isize, a1, a2, a3),
        SYS_CLOSE => fs::sys_close(a0),
        SYS_LSEEK => fs::sys_lseek(a0, a1 as i64, a2),
        SYS_DUP => fs::sys_dup(a0),
        SYS_DUP3 => fs::sys_dup3(a0, a1, a2),
        SYS_FCNTL => fs::sys_fcntl(a0, a1, a2),
        SYS_IOCTL => fs::sys_ioctl(a0, a1, a2),
        SYS_FSTAT => fs::sys_fstat(a0, a1),
        SYS_NEWFSTATAT => fs::sys_newfstatat(a0 as isize, a1, a2, a3),
        SYS_FACCESSAT => fs::sys_faccessat(a0 as isize, a1, a2),
        SYS_GETDENTS64 => fs::sys_getdents64(a0, a1, a2),
        SYS_CHDIR => fs::sys_chdir(a1),
        SYS_FCHDIR => fs::sys_fchdir(a0),
        SYS_GETCWD => fs::sys_getcwd(a0, a1),
        SYS_MKDIRAT => fs::sys_mkdirat(a0 as isize, a1, a2),
        SYS_UNLINKAT => fs::sys_unlinkat(a0 as isize, a1, a2),
        SYS_RENAMEAT => fs::sys_renameat(a0 as isize, a1, a2 as isize, a3),
        SYS_RENAMEAT2 => fs::sys_renameat2(a0 as isize, a1, a2 as isize, a3, a4),
        SYS_TRUNCATE => fs::sys_truncate(a1, a2 as i64),
        SYS_FTRUNCATE => fs::sys_ftruncate(a0, a1 as i64),
        SYS_FSYNC | SYS_FDATASYNC => 0,
        SYS_STATFS => fs::sys_statfs(a1, a2),
        SYS_FSTATFS => fs::sys_fstatfs(a0, a1),
        SYS_SENDFILE => fs::sys_sendfile(a0, a1, a2, a3),
        SYS_UTIMENSAT => 0,
        SYS_FCHMOD => 0,
        SYS_FCHMODAT => 0,
        SYS_FCHOWN => 0,
        SYS_FCHOWNAT => 0,
        SYS_PIPE2 => fs::sys_pipe2(a0, a1),
        SYS_EVENTFD2 => fs::sys_eventfd2(a0, a1),
        SYS_READLINKAT => fs::sys_readlinkat(a0 as isize, a1, a2, a3),

        SYS_EXIT => process::sys_exit(a0 as i32),
        SYS_EXIT_GROUP => process::sys_exit(a0 as i32),
        SYS_GETPID => process::sys_getpid(),
        SYS_GETPPID => process::sys_getppid(),
        SYS_GETTID => process::sys_getpid(),
        SYS_CLONE => process::sys_clone(a0),
        SYS_CLONE3 => -38,
        SYS_EXECVE => process::sys_execve(a0, a1, a2),
        SYS_EXECVEAT => -38,
        SYS_WAIT4 => process::sys_wait4(a0 as isize, a1, a2 as i32, a3),
        SYS_WAITID => process::sys_waitid(a0, a1 as isize, a2, a3 as i32),
        SYS_KILL => process::sys_kill(a0 as isize, a1),
        SYS_TKILL => process::sys_kill(a0 as isize, a1),
        SYS_TGKILL => process::sys_kill(a0 as isize, a2),
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => 0,
        SYS_GETGROUPS => process::sys_getgroups(a0, a1),
        SYS_SETGROUPS => 0,
        SYS_UNAME => process::sys_uname(a0),
        SYS_SETHOSTNAME => 0,
        SYS_GETRLIMIT => process::sys_getrlimit(a0, a1),
        SYS_SETRLIMIT => 0,
        SYS_PRlimit64 => process::sys_prlimit64(a0 as isize, a1, a2, a3),
        SYS_GETRUSAGE => process::sys_getrusage(a0, a1),
        SYS_UMASK => process::sys_umask(a0),
        SYS_PRCTL => process::sys_prctl(a0, a1, a2),
        SYS_PERSONALITY => 0,
        SYS_GETTIMEOFDAY => process::sys_gettimeofday(a0, a1),
        SYS_TIMES => process::sys_times(a0),
        SYS_SYSINFO => process::sys_sysinfo(a0),
        SYS_CLOCK_GETTIME => process::sys_clock_gettime(a0, a1),
        SYS_CLOCK_GETRES => process::sys_clock_getres(a0, a1),
        SYS_CLOCK_NANOSLEEP => process::sys_clock_nanosleep(a0, a1, a2, a3),
        SYS_NANOSLEEP => process::sys_nanosleep(a0, a1),
        SYS_SCHED_YIELD => process::sys_sched_yield(),
        SYS_SCHED_GETAFFINITY => process::sys_sched_getaffinity(a0, a1, a2),
        SYS_SCHED_SETAFFINITY => 0,
        SYS_GETRANDOM => process::sys_getrandom(a0, a1, a2),
        SYS_SET_TID_ADDRESS => process::sys_set_tid_address(a0),
        SYS_SET_ROBUST_LIST => 0,
        SYS_GET_ROBUST_LIST => 0,
        SYS_FUTEX => process::sys_futex(a0, a1, a2, a3, a4),
        SYS_RSEQ => -38,
        SYS_SETPGID | SYS_GETPGID | SYS_SETSID => 0,
        SYS_GETCPU => 0,
        SYS_MEMBARRIER => 0,

        SYS_MMAP => mem::sys_mmap(a0, a1, a2, a3, a4, a5),
        SYS_MUNMAP => mem::sys_munmap(a0, a1),
        SYS_MPROTECT => mem::sys_mprotect(a0, a1, a2),
        SYS_BRK => mem::sys_brk(a0),
        SYS_MREMAP => mem::sys_mremap(a0, a1, a2, a3),
        SYS_MADVISE => 0,
        SYS_MSYNC => 0,
        SYS_MLOCK | SYS_MUNLOCK | SYS_MLOCKALL | SYS_MUNLOCKALL => 0,

        SYS_RT_SIGACTION => signal::sys_rt_sigaction(a0, a1, a2, a3),
        SYS_RT_SIGPROCMASK => signal::sys_rt_sigprocmask(a0, a1, a2, a3),
        SYS_RT_SIGPENDING => signal::sys_rt_sigpending(a0, a1),
        SYS_RT_SIGRETURN => signal::sys_rt_sigreturn(),
        SYS_SIGALTSTACK => signal::sys_sigaltstack(a0, a1),
        SYS_RT_SIGTIMEDWAIT => -38,

        SYS_SOCKET => socket::sys_socket(a0 as i32, a1 as i32, a2 as i32),
        SYS_SOCKETPAIR => socket::sys_socketpair(a0 as i32, a1 as i32, a2 as i32, a3),
        SYS_BIND => socket::sys_bind(a0, a1, a2),
        SYS_LISTEN => socket::sys_listen(a0, a1 as i32),
        SYS_ACCEPT => socket::sys_accept(a0, a1, a2, 0),
        SYS_ACCEPT4 => socket::sys_accept(a0, a1, a2, a3),
        SYS_CONNECT => socket::sys_connect(a0, a1, a2),
        SYS_GETSOCKNAME => socket::sys_getsockname(a0, a1, a2),
        SYS_GETPEERNAME => socket::sys_getpeername(a0, a1, a2),
        SYS_SENDTO => socket::sys_sendto(a0, a1, a2, a3, a4, a5),
        SYS_RECVFROM => socket::sys_recvfrom(a0, a1, a2, a3, a4, a5),
        SYS_SETSOCKOPT => socket::sys_setsockopt(a0, a1, a2, a3, a4),
        SYS_GETSOCKOPT => socket::sys_getsockopt(a0, a1, a2, a3, a4),
        SYS_SHUTDOWN => socket::sys_shutdown(a0, a1 as i32),
        SYS_SENDMSG => socket::sys_sendmsg(a0, a1, a2),
        SYS_RECVMSG => socket::sys_recvmsg(a0, a1, a2),

        SYS_EPOLL_CREATE1 => crate::epoll::sys_epoll_create1(a0),
        SYS_EPOLL_CTL => crate::epoll::sys_epoll_ctl(a0, a1, a2, a3),
        SYS_EPOLL_PWAIT => crate::epoll::sys_epoll_pwait(a0, a1, a2, a3, a4),

        _ => {
            crate::console::kprintln!(
                "[sys] pid={} unimplemented syscall {} ({:#x}) args={:#x} {:#x} {:#x} {:#x} {:#x} {:#x}",
                crate::task::current_pid(),
                num,
                num,
                a0,
                a1,
                a2,
                a3,
                a4,
                a5
            );
            -38 // ENOSYS
        }
    }
}

// ---------- user memory helpers ----------

pub fn read_user(addr: usize, len: usize) -> Result<Vec<u8>, i32> {
    let t = crate::task::current();
    let mm = unsafe { &t.as_ref().unwrap().mm };
    if !mm.check_range(addr, len, false) {
        return Err(-14); // EFAULT
    }
    let mut v = alloc::vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(addr as *const u8, v.as_mut_ptr(), len);
    }
    Ok(v)
}

pub fn read_user_partial(addr: usize, len: usize) -> Result<Vec<u8>, i32> {
    read_user(addr, len)
}

pub fn write_user(addr: usize, data: &[u8]) -> Result<(), i32> {
    let t = crate::task::current();
    let mm = unsafe { &t.as_ref().unwrap().mm };
    if !mm.check_range(addr, data.len(), true) {
        return Err(-14);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
    }
    Ok(())
}

/// Read a NUL-terminated string from user memory.
pub fn read_cstr(addr: usize, max: usize) -> Result<alloc::string::String, i32> {
    let t = crate::task::current();
    let mm = unsafe { &t.as_ref().unwrap().mm };
    let mut len = 0usize;
    while len < max {
        if !mm.check_range(addr + len, 1, false) {
            return Err(-14);
        }
        let b = unsafe { *(addr as *const u8).add(len) };
        if b == 0 {
            break;
        }
        len += 1;
    }
    if len >= max {
        return Err(-36); // ENAMETOOLONG
    }
    let bytes = read_user(addr, len)?;
    Ok(alloc::string::String::from_utf8_lossy(&bytes).into_owned())
}

/// Read an array of user pointers (e.g. argv) terminated by NULL.
pub fn read_ptr_array(addr: usize, max: usize) -> Result<Vec<usize>, i32> {
    let mut v = Vec::new();
    for i in 0..max {
        let p = read_user(addr + i * 8, 8)?;
        let val = u64::from_le_bytes(p[..8].try_into().unwrap()) as usize;
        if val == 0 {
            return Ok(v);
        }
        v.push(val);
    }
    Ok(v)
}
