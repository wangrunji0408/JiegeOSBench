//! Linux system call dispatch.
pub mod fs;
pub mod futex;
pub mod misc;
pub mod mm;
pub mod net;
pub mod process;
pub mod signal;

use crate::abi::nr::*;
use crate::abi::*;
use crate::task::current;
use crate::task::signal::RestartKind;
use crate::trap::TrapFrame;

fn name(nr: usize) -> &'static str {
    match nr {
        GETCWD => "getcwd",
        EVENTFD2 => "eventfd2",
        EPOLL_CREATE1 => "epoll_create1",
        EPOLL_CTL => "epoll_ctl",
        EPOLL_PWAIT => "epoll_pwait",
        DUP => "dup",
        DUP3 => "dup3",
        FCNTL => "fcntl",
        IOCTL => "ioctl",
        MKDIRAT => "mkdirat",
        UNLINKAT => "unlinkat",
        SYMLINKAT => "symlinkat",
        LINKAT => "linkat",
        FTRUNCATE => "ftruncate",
        FACCESSAT => "faccessat",
        CHDIR => "chdir",
        FCHDIR => "fchdir",
        FCHMOD => "fchmod",
        FCHMODAT => "fchmodat",
        FCHOWNAT => "fchownat",
        FCHOWN => "fchown",
        OPENAT => "openat",
        CLOSE => "close",
        PIPE2 => "pipe2",
        GETDENTS64 => "getdents64",
        LSEEK => "lseek",
        READ => "read",
        WRITE => "write",
        READV => "readv",
        WRITEV => "writev",
        PREAD64 => "pread64",
        PWRITE64 => "pwrite64",
        SENDFILE => "sendfile",
        PPOLL => "ppoll",
        PSELECT6 => "pselect6",
        READLINKAT => "readlinkat",
        NEWFSTATAT => "fstatat",
        FSTAT => "fstat",
        FSYNC => "fsync",
        FDATASYNC => "fdatasync",
        UTIMENSAT => "utimensat",
        EXIT => "exit",
        EXIT_GROUP => "exit_group",
        SET_TID_ADDRESS => "set_tid_address",
        FUTEX => "futex",
        SET_ROBUST_LIST => "set_robust_list",
        NANOSLEEP => "nanosleep",
        CLOCK_GETTIME => "clock_gettime",
        CLOCK_NANOSLEEP => "clock_nanosleep",
        SCHED_YIELD => "sched_yield",
        SCHED_GETAFFINITY => "sched_getaffinity",
        KILL => "kill",
        TKILL => "tkill",
        TGKILL => "tgkill",
        SIGALTSTACK => "sigaltstack",
        RT_SIGSUSPEND => "rt_sigsuspend",
        RT_SIGACTION => "rt_sigaction",
        RT_SIGPROCMASK => "rt_sigprocmask",
        RT_SIGPENDING => "rt_sigpending",
        RT_SIGTIMEDWAIT => "rt_sigtimedwait",
        RT_SIGRETURN => "rt_sigreturn",
        SETGID => "setgid",
        SETUID => "setuid",
        SETPGID => "setpgid",
        GETPGID => "getpgid",
        SETSID => "setsid",
        SETGROUPS => "setgroups",
        UNAME => "uname",
        GETRLIMIT => "getrlimit",
        SETRLIMIT => "setrlimit",
        UMASK => "umask",
        PRCTL => "prctl",
        GETTIMEOFDAY => "gettimeofday",
        GETPID => "getpid",
        GETPPID => "getppid",
        GETUID => "getuid",
        GETEUID => "geteuid",
        GETGID => "getgid",
        GETEGID => "getegid",
        GETTID => "gettid",
        SYSINFO => "sysinfo",
        SOCKET => "socket",
        SOCKETPAIR => "socketpair",
        BIND => "bind",
        LISTEN => "listen",
        ACCEPT => "accept",
        CONNECT => "connect",
        GETSOCKNAME => "getsockname",
        GETPEERNAME => "getpeername",
        SENDTO => "sendto",
        RECVFROM => "recvfrom",
        SETSOCKOPT => "setsockopt",
        GETSOCKOPT => "getsockopt",
        SHUTDOWN => "shutdown",
        SENDMSG => "sendmsg",
        RECVMSG => "recvmsg",
        BRK => "brk",
        MUNMAP => "munmap",
        MREMAP => "mremap",
        CLONE => "clone",
        EXECVE => "execve",
        MMAP => "mmap",
        MPROTECT => "mprotect",
        MADVISE => "madvise",
        ACCEPT4 => "accept4",
        WAIT4 => "wait4",
        PRLIMIT64 => "prlimit64",
        GETRANDOM => "getrandom",
        STATX => "statx",
        IO_SETUP => "io_setup",
        MEMBARRIER => "membarrier",
        RSEQ => "rseq",
        CLONE3 => "clone3",
        RISCV_HWPROBE => "riscv_hwprobe",
        _ => "?",
    }
}

fn restart_kind(nr: usize) -> Option<RestartKind> {
    match nr {
        READ | WRITE | READV | WRITEV | PREAD64 | PWRITE64 | WAIT4 | WAITID | ACCEPT | ACCEPT4 | RECVFROM | SENDTO
        | RECVMSG | SENDMSG | CONNECT | FUTEX | SENDFILE | FLOCK => Some(RestartKind::Always),
        PPOLL | PSELECT6 | RT_SIGSUSPEND | RT_SIGTIMEDWAIT | NANOSLEEP | CLOCK_NANOSLEEP | EPOLL_PWAIT
        | EPOLL_PWAIT2 => Some(RestartKind::NoHand),
        _ => None,
    }
}

pub fn dispatch(tf: &mut TrapFrame) {
    let nr = tf.x[17];
    let args = tf.syscall_args();
    let strace =
        (crate::config::STRACE || crate::fs::devices::strace_enabled(current().pid)) && nr != SENDFILE && nr != WRITEV;
    if strace {
        let t = current();
        crate::println!(
            "[{}] {} {}({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            t.pid,
            crate::time::monotonic_ns() / 1_000_000,
            name(nr),
            args[0],
            args[1],
            args[2],
            args[3],
            args[4],
            args[5]
        );
    }
    let result: SysResult = match nr {
        // fs
        GETCWD => fs::sys_getcwd(args[0], args[1]),
        EVENTFD2 => fs::sys_eventfd2(args[0] as u32, args[1] as u32),
        EPOLL_CREATE1 => fs::sys_epoll_create1(args[0] as u32),
        EPOLL_CTL => fs::sys_epoll_ctl(args[0] as i32, args[1] as i32, args[2] as i32, args[3]),
        EPOLL_PWAIT => fs::sys_epoll_pwait(args[0] as i32, args[1], args[2] as i32, args[3] as i32, args[4]),
        EPOLL_PWAIT2 => fs::sys_epoll_pwait2(args[0] as i32, args[1], args[2] as i32, args[3], args[4]),
        DUP => fs::sys_dup(args[0] as i32),
        DUP3 => fs::sys_dup3(args[0] as i32, args[1] as i32, args[2] as u32),
        FCNTL => fs::sys_fcntl(args[0] as i32, args[1] as u32, args[2]),
        IOCTL => fs::sys_ioctl(args[0] as i32, args[1] as u32, args[2]),
        MKDIRAT => fs::sys_mkdirat(args[0] as i32, args[1], args[2] as u32),
        MKNODAT => fs::sys_mknodat(args[0] as i32, args[1], args[2] as u32, args[3]),
        UNLINKAT => fs::sys_unlinkat(args[0] as i32, args[1], args[2] as u32),
        SYMLINKAT => fs::sys_symlinkat(args[0], args[1] as i32, args[2]),
        LINKAT => fs::sys_linkat(args[0] as i32, args[1], args[2] as i32, args[3], args[4] as u32),
        RENAMEAT2 => fs::sys_renameat(args[0] as i32, args[1], args[2] as i32, args[3]),
        TRUNCATE => fs::sys_truncate(args[0], args[1] as i64),
        FTRUNCATE => fs::sys_ftruncate(args[0] as i32, args[1] as i64),
        FACCESSAT | FACCESSAT2 => fs::sys_faccessat(args[0] as i32, args[1], args[2] as u32, args[3] as u32),
        CHDIR => fs::sys_chdir(args[0]),
        FCHDIR => fs::sys_fchdir(args[0] as i32),
        FCHMOD => fs::sys_fchmod(args[0] as i32, args[1] as u32),
        FCHMODAT | FCHMODAT2 => fs::sys_fchmodat(args[0] as i32, args[1], args[2] as u32),
        FCHOWNAT => fs::sys_fchownat(args[0] as i32, args[1], args[2] as u32, args[3] as u32, args[4] as u32),
        FCHOWN => fs::sys_fchown(args[0] as i32, args[1] as u32, args[2] as u32),
        OPENAT => fs::sys_openat(args[0] as i32, args[1], args[2] as u32, args[3] as u32),
        CLOSE => fs::sys_close(args[0] as i32),
        CLOSE_RANGE => fs::sys_close_range(args[0] as u32, args[1] as u32, args[2] as u32),
        PIPE2 => fs::sys_pipe2(args[0], args[1] as u32),
        GETDENTS64 => fs::sys_getdents64(args[0] as i32, args[1], args[2]),
        LSEEK => fs::sys_lseek(args[0] as i32, args[1] as i64, args[2] as i32),
        READ => fs::sys_read(args[0] as i32, args[1], args[2]),
        WRITE => fs::sys_write(args[0] as i32, args[1], args[2]),
        READV => fs::sys_readv(args[0] as i32, args[1], args[2]),
        WRITEV => fs::sys_writev(args[0] as i32, args[1], args[2]),
        PREAD64 => fs::sys_pread64(args[0] as i32, args[1], args[2], args[3] as u64),
        PWRITE64 => fs::sys_pwrite64(args[0] as i32, args[1], args[2], args[3] as u64),
        PREADV | PREADV2 => fs::sys_preadv(args[0] as i32, args[1], args[2], args[3] as u64),
        PWRITEV | PWRITEV2 => fs::sys_pwritev(args[0] as i32, args[1], args[2], args[3] as u64),
        SENDFILE => fs::sys_sendfile(args[0] as i32, args[1] as i32, args[2], args[3]),
        PPOLL => fs::sys_ppoll(args[0], args[1], args[2], args[3]),
        PSELECT6 => fs::sys_pselect6(args[0] as i32, args[1], args[2], args[3], args[4], args[5]),
        READLINKAT => fs::sys_readlinkat(args[0] as i32, args[1], args[2], args[3]),
        NEWFSTATAT => fs::sys_fstatat(args[0] as i32, args[1], args[2], args[3] as u32),
        FSTAT => fs::sys_fstat(args[0] as i32, args[1]),
        STATX => fs::sys_statx(args[0] as i32, args[1], args[2] as u32, args[3] as u32, args[4]),
        SYNC | FSYNC | FDATASYNC | SYNCFS => Ok(0),
        UTIMENSAT => fs::sys_utimensat(args[0] as i32, args[1], args[2], args[3] as u32),
        STATFS => fs::sys_statfs(args[0], args[1]),
        FSTATFS => fs::sys_fstatfs(args[0] as i32, args[1]),
        FLOCK => Ok(0),
        FALLOCATE => fs::sys_fallocate(args[0] as i32, args[1] as i32, args[2] as i64, args[3] as i64),
        FADVISE64 => Ok(0),
        READAHEAD => Ok(0),
        COPY_FILE_RANGE => fs::sys_copy_file_range(args[0] as i32, args[1], args[2] as i32, args[3], args[4]),
        UMASK => fs::sys_umask(args[0] as u32),
        SPLICE => Err(EINVAL),
        MEMFD_CREATE => fs::sys_memfd_create(args[0], args[1] as u32),
        // process
        EXIT => process::sys_exit(args[0] as i32),
        EXIT_GROUP => process::sys_exit_group(args[0] as i32),
        SET_TID_ADDRESS => process::sys_set_tid_address(args[0]),
        SET_ROBUST_LIST => process::sys_set_robust_list(args[0], args[1]),
        GET_ROBUST_LIST => Err(ENOSYS),
        CLONE => process::sys_clone(args[0] as u64, args[1], args[2], args[3], args[4]),
        CLONE3 => Err(ENOSYS),
        EXECVE => process::sys_execve(args[0], args[1], args[2]),
        WAIT4 => process::sys_wait4(args[0] as i32, args[1], args[2] as i32, args[3]),
        WAITID => process::sys_waitid(args[0] as i32, args[1] as i32, args[2], args[3] as i32),
        GETPID => process::sys_getpid(),
        GETPPID => process::sys_getppid(),
        GETTID => process::sys_gettid(),
        GETUID | GETEUID => process::sys_getuid(),
        GETGID | GETEGID => process::sys_getgid(),
        SETUID | SETREUID | SETRESUID | SETFSUID => process::sys_setuid(args[0] as u32),
        SETGID | SETREGID | SETRESGID | SETFSGID => process::sys_setgid(args[0] as u32),
        GETRESUID => process::sys_getresuid(args[0], args[1], args[2]),
        GETRESGID => process::sys_getresgid(args[0], args[1], args[2]),
        SETGROUPS => Ok(0),
        GETGROUPS => Ok(0),
        SETPGID => process::sys_setpgid(args[0] as i32, args[1] as i32),
        GETPGID => process::sys_getpgid(args[0] as i32),
        GETSID => process::sys_getsid(args[0] as i32),
        SETSID => process::sys_setsid(),
        SCHED_YIELD => process::sys_sched_yield(),
        SCHED_GETAFFINITY => process::sys_sched_getaffinity(args[0] as i32, args[1], args[2]),
        SCHED_SETAFFINITY => Ok(0),
        SCHED_GETSCHEDULER => Ok(0),
        SCHED_SETSCHEDULER | SCHED_SETPARAM => Ok(0),
        SCHED_GETPARAM => Ok(0),
        SCHED_GET_PRIORITY_MAX => Ok(0),
        SCHED_GET_PRIORITY_MIN => Ok(0),
        SCHED_SETATTR | SCHED_GETATTR => Err(ENOSYS),
        GETPRIORITY => Ok(20),
        SETPRIORITY => Ok(0),
        IOPRIO_SET => Ok(0),
        GETRLIMIT => process::sys_getrlimit(args[0] as u32, args[1]),
        SETRLIMIT => process::sys_setrlimit(args[0] as u32, args[1]),
        PRLIMIT64 => process::sys_prlimit64(args[0] as i32, args[1] as u32, args[2], args[3]),
        GETRUSAGE => process::sys_getrusage(args[0] as i32, args[1]),
        TIMES => process::sys_times(args[0]),
        PRCTL => process::sys_prctl(args[0] as i32, args[1], args[2], args[3], args[4]),
        CAPGET => Ok(0),
        CAPSET => Ok(0),
        PERSONALITY => Ok(0),
        // signals
        KILL => signal::sys_kill(args[0] as i32, args[1] as i32),
        TKILL => signal::sys_tkill(args[0] as i32, args[1] as i32),
        TGKILL => signal::sys_tgkill(args[0] as i32, args[1] as i32, args[2] as i32),
        SIGALTSTACK => signal::sys_sigaltstack(args[0], args[1]),
        RT_SIGSUSPEND => signal::sys_rt_sigsuspend(args[0], args[1]),
        RT_SIGACTION => signal::sys_rt_sigaction(args[0] as i32, args[1], args[2], args[3]),
        RT_SIGPROCMASK => signal::sys_rt_sigprocmask(args[0] as i32, args[1], args[2], args[3]),
        RT_SIGPENDING => signal::sys_rt_sigpending(args[0], args[1]),
        RT_SIGTIMEDWAIT => signal::sys_rt_sigtimedwait(args[0], args[1], args[2], args[3]),
        RT_SIGQUEUEINFO => signal::sys_kill(args[0] as i32, args[1] as i32),
        RT_SIGRETURN => signal::sys_rt_sigreturn(tf),
        // memory
        BRK => mm::sys_brk(args[0]),
        MUNMAP => mm::sys_munmap(args[0], args[1]),
        MREMAP => mm::sys_mremap(args[0], args[1], args[2], args[3] as u32, args[4]),
        MMAP => mm::sys_mmap(args[0], args[1], args[2] as u32, args[3] as u32, args[4] as i32, args[5] as u64),
        MPROTECT => mm::sys_mprotect(args[0], args[1], args[2] as u32),
        MADVISE => mm::sys_madvise(args[0], args[1], args[2] as i32),
        MSYNC => Ok(0),
        MLOCK | MUNLOCK | MLOCKALL | MUNLOCKALL | MLOCK2 => Ok(0),
        MINCORE => Err(ENOSYS),
        MEMBARRIER => Ok(0),
        // time
        NANOSLEEP => misc::sys_nanosleep(args[0], args[1]),
        CLOCK_NANOSLEEP => misc::sys_clock_nanosleep(args[0] as i32, args[1] as i32, args[2], args[3]),
        CLOCK_GETTIME => misc::sys_clock_gettime(args[0] as i32, args[1]),
        CLOCK_GETRES => misc::sys_clock_getres(args[0] as i32, args[1]),
        CLOCK_SETTIME => misc::sys_clock_settime(args[0] as i32, args[1]),
        GETTIMEOFDAY => misc::sys_gettimeofday(args[0], args[1]),
        SETTIMEOFDAY => Ok(0),
        SETITIMER => misc::sys_setitimer(args[0] as i32, args[1], args[2]),
        GETITIMER => misc::sys_getitimer(args[0] as i32, args[1]),
        TIMER_CREATE | TIMER_SETTIME | TIMER_DELETE => Err(ENOSYS),
        TIMERFD_CREATE | TIMERFD_SETTIME | TIMERFD_GETTIME => Err(ENOSYS),
        // misc
        UNAME => misc::sys_uname(args[0]),
        SETHOSTNAME => Ok(0),
        SETDOMAINNAME => Ok(0),
        SYSINFO => misc::sys_sysinfo(args[0]),
        GETRANDOM => misc::sys_getrandom(args[0], args[1], args[2] as u32),
        SYSLOG => Ok(0),
        GETCPU => misc::sys_getcpu(args[0], args[1]),
        RISCV_HWPROBE => Err(ENOSYS),
        RISCV_FLUSH_ICACHE => {
            unsafe { core::arch::asm!("fence.i") };
            Ok(0)
        }
        RSEQ => Err(ENOSYS),
        SECCOMP => Err(ENOSYS),
        REBOOT => misc::sys_reboot(args[0] as u32, args[1] as u32, args[2] as u32),
        MOUNT | UMOUNT2 | CHROOT => Ok(0),
        IO_SETUP | IO_DESTROY | IO_SUBMIT | IO_GETEVENTS => Err(ENOSYS),
        IO_URING_SETUP | IO_URING_ENTER | IO_URING_REGISTER => Err(ENOSYS),
        INOTIFY_INIT1 | FANOTIFY_INIT => Err(ENOSYS),
        SIGNALFD4 => Err(ENOSYS),
        SETXATTR | GETXATTR | LGETXATTR | FGETXATTR | LISTXATTR => Err(ENOTSUP_XATTR),
        SHMGET | SHMAT | SHMDT | SHMCTL | SEMGET | MSGGET | MQ_OPEN => Err(ENOSYS),
        PTRACE => Err(EPERM),
        ACCT | SWAPON | KEXEC_FILE_LOAD | FINIT_MODULE => Err(EPERM),
        // net
        SOCKET => net::sys_socket(args[0] as u32, args[1] as u32, args[2] as u32),
        SOCKETPAIR => net::sys_socketpair(args[0] as u32, args[1] as u32, args[2] as u32, args[3]),
        BIND => net::sys_bind(args[0] as i32, args[1], args[2] as u32),
        LISTEN => net::sys_listen(args[0] as i32, args[1] as i32),
        ACCEPT => net::sys_accept4(args[0] as i32, args[1], args[2], 0),
        ACCEPT4 => net::sys_accept4(args[0] as i32, args[1], args[2], args[3] as u32),
        CONNECT => net::sys_connect(args[0] as i32, args[1], args[2] as u32),
        GETSOCKNAME => net::sys_getsockname(args[0] as i32, args[1], args[2]),
        GETPEERNAME => net::sys_getpeername(args[0] as i32, args[1], args[2]),
        SENDTO => net::sys_sendto(args[0] as i32, args[1], args[2], args[3] as u32, args[4], args[5] as u32),
        RECVFROM => net::sys_recvfrom(args[0] as i32, args[1], args[2], args[3] as u32, args[4], args[5]),
        SETSOCKOPT => net::sys_setsockopt(args[0] as i32, args[1] as i32, args[2] as i32, args[3], args[4] as u32),
        GETSOCKOPT => net::sys_getsockopt(args[0] as i32, args[1] as i32, args[2] as i32, args[3], args[4]),
        SHUTDOWN => net::sys_shutdown(args[0] as i32, args[1] as i32),
        SENDMSG => net::sys_sendmsg(args[0] as i32, args[1], args[2] as u32),
        RECVMSG => net::sys_recvmsg(args[0] as i32, args[1], args[2] as u32),
        SENDMMSG | RECVMMSG => Err(ENOSYS),
        FUTEX => futex::sys_futex(args[0], args[1] as i32, args[2] as u32, args[3], args[4], args[5] as u32),
        FUTEX_WAITV => Err(ENOSYS),
        _ => {
            klog!("pid {}: unimplemented syscall {} ({})", current().pid, nr, name(nr));
            Err(ENOSYS)
        }
    };
    match result {
        Ok(v) => {
            if strace {
                crate::println!("[{}]   {} = {:#x}", current().pid, name(nr), v);
            }
            tf.set_a0(v);
        }
        Err(EINTR) if restart_kind(nr).is_some() => {
            let t = current();
            t.inner.lock().syscall_restart = Some((args[0], restart_kind(nr).unwrap()));
            tf.set_a0((-EINTR) as isize as usize);
        }
        Err(e) => {
            if strace {
                crate::println!("[{}]   {} = -{}", current().pid, name(nr), e);
            }
            tf.set_a0((-e) as isize as usize);
        }
    }
}

const ENOTSUP_XATTR: i32 = EOPNOTSUPP;
