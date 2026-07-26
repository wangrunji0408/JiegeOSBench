//! Linux syscall dispatch.

pub mod fs_ops;
pub mod mem_ops;
pub mod misc_ops;
pub mod net_ops;
pub mod number;
pub mod poll_ops;
pub mod proc_ops;
pub mod signal_ops;

use crate::fs::Result;
use crate::trap::TrapContext;
use number as sys;

/// Sentinel meaning "do not overwrite a0" (`execve` and `rt_sigreturn` set it).
pub const SKIP_RETURN: isize = isize::MIN;

/// Dispatch a syscall. Returns the value to place in a0.
pub fn dispatch(cx: &mut TrapContext) -> isize {
    let n = cx.syscall_number();
    let a = [
        cx.arg(0),
        cx.arg(1),
        cx.arg(2),
        cx.arg(3),
        cx.arg(4),
        cx.arg(5),
    ];

    let result: Result<isize> = match n {
        // ---- file I/O ----
        sys::READ => fs_ops::sys_read(a[0] as i32, a[1], a[2]),
        sys::WRITE => fs_ops::sys_write(a[0] as i32, a[1], a[2]),
        sys::READV => fs_ops::sys_readv(a[0] as i32, a[1], a[2]),
        sys::WRITEV => fs_ops::sys_writev(a[0] as i32, a[1], a[2]),
        sys::PREAD64 => fs_ops::sys_pread(a[0] as i32, a[1], a[2], a[3] as i64),
        sys::PWRITE64 => fs_ops::sys_pwrite(a[0] as i32, a[1], a[2], a[3] as i64),
        sys::OPENAT => fs_ops::sys_openat(a[0] as i32, a[1], a[2] as u32, a[3] as u32),
        sys::CLOSE => fs_ops::sys_close(a[0] as i32),
        sys::LSEEK => fs_ops::sys_lseek(a[0] as i32, a[1] as i64, a[2] as u32),
        sys::DUP => fs_ops::sys_dup(a[0] as i32),
        sys::DUP3 => fs_ops::sys_dup3(a[0] as i32, a[1] as i32, a[2] as u32),
        sys::FCNTL => fs_ops::sys_fcntl(a[0] as i32, a[1] as u32, a[2]),
        sys::IOCTL => fs_ops::sys_ioctl(a[0] as i32, a[1], a[2]),
        sys::FSTAT => fs_ops::sys_fstat(a[0] as i32, a[1]),
        sys::FSTATAT => fs_ops::sys_fstatat(a[0] as i32, a[1], a[2], a[3]),
        sys::STATX => fs_ops::sys_statx(a[0] as i32, a[1], a[2], a[3] as u32, a[4]),
        sys::GETDENTS64 => fs_ops::sys_getdents64(a[0] as i32, a[1], a[2]),
        sys::MKDIRAT => fs_ops::sys_mkdirat(a[0] as i32, a[1], a[2] as u32),
        sys::MKNODAT => fs_ops::sys_mknodat(a[0] as i32, a[1], a[2] as u32, a[3]),
        sys::UNLINKAT => fs_ops::sys_unlinkat(a[0] as i32, a[1], a[2] as u32),
        sys::SYMLINKAT => fs_ops::sys_symlinkat(a[0], a[1] as i32, a[2]),
        sys::LINKAT => fs_ops::sys_linkat(a[0] as i32, a[1], a[2] as i32, a[3], a[4] as u32),
        sys::RENAMEAT => fs_ops::sys_renameat(a[0] as i32, a[1], a[2] as i32, a[3]),
        sys::RENAMEAT2 => fs_ops::sys_renameat(a[0] as i32, a[1], a[2] as i32, a[3]),
        sys::READLINKAT => fs_ops::sys_readlinkat(a[0] as i32, a[1], a[2], a[3]),
        sys::FACCESSAT => fs_ops::sys_faccessat(a[0] as i32, a[1], a[2] as u32),
        sys::FACCESSAT2 => fs_ops::sys_faccessat(a[0] as i32, a[1], a[2] as u32),
        sys::TRUNCATE => fs_ops::sys_truncate(a[0], a[1]),
        sys::FTRUNCATE => fs_ops::sys_ftruncate(a[0] as i32, a[1]),
        sys::GETCWD => fs_ops::sys_getcwd(a[0], a[1]),
        sys::CHDIR => fs_ops::sys_chdir(a[0]),
        sys::FCHDIR => fs_ops::sys_fchdir(a[0] as i32),
        sys::CHROOT => Ok(0),
        sys::FCHMOD => fs_ops::sys_fchmod(a[0] as i32, a[1] as u32),
        sys::FCHMODAT => fs_ops::sys_fchmodat(a[0] as i32, a[1], a[2] as u32),
        sys::FCHOWN => fs_ops::sys_fchown(a[0] as i32, a[1] as u32, a[2] as u32),
        sys::FCHOWNAT => fs_ops::sys_fchownat(a[0] as i32, a[1], a[2] as u32, a[3] as u32, a[4]),
        sys::PIPE2 => fs_ops::sys_pipe2(a[0], a[1] as u32),
        sys::SENDFILE => fs_ops::sys_sendfile(a[0] as i32, a[1] as i32, a[2], a[3]),
        sys::STATFS => fs_ops::sys_statfs(a[0], a[1]),
        sys::FSTATFS => fs_ops::sys_fstatfs(a[0] as i32, a[1]),
        sys::UTIMENSAT => Ok(0),
        sys::SYNC | sys::FSYNC | sys::FDATASYNC => Ok(0),
        sys::FADVISE64 | sys::READAHEAD => Ok(0),
        sys::MEMFD_CREATE => fs_ops::sys_memfd_create(a[0], a[1] as u32),
        sys::CLOSE_RANGE => fs_ops::sys_close_range(a[0] as u32, a[1] as u32, a[2] as u32),
        sys::MOUNT | sys::UMOUNT2 => Ok(0),

        // ---- memory ----
        sys::BRK => mem_ops::sys_brk(a[0]),
        sys::MMAP => mem_ops::sys_mmap(a[0], a[1], a[2] as u32, a[3] as u32, a[4] as i32, a[5]),
        sys::MUNMAP => mem_ops::sys_munmap(a[0], a[1]),
        sys::MPROTECT => mem_ops::sys_mprotect(a[0], a[1], a[2] as u32),
        sys::MREMAP => mem_ops::sys_mremap(a[0], a[1], a[2], a[3] as u32, a[4]),
        sys::MADVISE => mem_ops::sys_madvise(a[0], a[1], a[2] as u32),
        sys::MSYNC => Ok(0),
        sys::MLOCK | sys::MUNLOCK | sys::MLOCKALL | sys::MUNLOCKALL => Ok(0),
        sys::SHMGET => mem_ops::sys_shmget(a[0], a[1], a[2] as u32),
        sys::SHMAT => mem_ops::sys_shmat(a[0] as i32, a[1], a[2] as u32),
        sys::SHMDT => mem_ops::sys_shmdt(a[0]),
        sys::SHMCTL => mem_ops::sys_shmctl(a[0] as i32, a[1] as u32, a[2]),

        // ---- process ----
        sys::CLONE => proc_ops::sys_clone(cx, a[0], a[1], a[2], a[3], a[4]),
        sys::CLONE3 => proc_ops::sys_clone3(cx, a[0], a[1]),
        sys::EXECVE => return proc_ops::sys_execve(cx, a[0], a[1], a[2]),
        sys::EXIT => proc_ops::sys_exit(a[0] as i32),
        sys::EXIT_GROUP => proc_ops::sys_exit_group(a[0] as i32),
        sys::WAIT4 => proc_ops::sys_wait4(a[0] as isize, a[1], a[2] as u32, a[3]),
        sys::GETPID => Ok(crate::task::current().pid() as isize),
        sys::GETPPID => Ok(crate::task::current().ppid() as isize),
        sys::GETTID => Ok(crate::task::current().tid as isize),
        sys::SET_TID_ADDRESS => proc_ops::sys_set_tid_address(a[0]),
        sys::SET_ROBUST_LIST => proc_ops::sys_set_robust_list(a[0], a[1]),
        sys::GET_ROBUST_LIST => proc_ops::sys_get_robust_list(a[0] as i32, a[1], a[2]),
        sys::FUTEX => proc_ops::sys_futex(a[0], a[1] as u32, a[2] as u32, a[3], a[4], a[5] as u32),
        sys::SCHED_YIELD => {
            crate::task::yield_now();
            Ok(0)
        }
        sys::GETUID => Ok(crate::task::current().uid() as isize),
        sys::GETEUID => Ok(crate::task::current().euid() as isize),
        sys::GETGID => Ok(crate::task::current().gid() as isize),
        sys::GETEGID => Ok(crate::task::current().egid() as isize),
        sys::SETUID => proc_ops::sys_setuid(a[0] as u32),
        sys::SETGID => proc_ops::sys_setgid(a[0] as u32),
        sys::SETREUID => proc_ops::sys_setreuid(a[0] as u32, a[1] as u32),
        sys::SETREGID => proc_ops::sys_setregid(a[0] as u32, a[1] as u32),
        sys::SETRESUID => proc_ops::sys_setresuid(a[0] as u32, a[1] as u32, a[2] as u32),
        sys::SETRESGID => proc_ops::sys_setresgid(a[0] as u32, a[1] as u32, a[2] as u32),
        sys::GETRESUID => proc_ops::sys_getresuid(a[0], a[1], a[2]),
        sys::GETRESGID => proc_ops::sys_getresgid(a[0], a[1], a[2]),
        sys::SETFSUID | sys::SETFSGID => Ok(0),
        sys::GETGROUPS => proc_ops::sys_getgroups(a[0] as i32, a[1]),
        sys::SETGROUPS => proc_ops::sys_setgroups(a[0] as i32, a[1]),
        sys::SETPGID => proc_ops::sys_setpgid(a[0], a[1]),
        sys::GETPGID => proc_ops::sys_getpgid(a[0]),
        sys::SETSID => proc_ops::sys_setsid(),
        sys::GETSID => proc_ops::sys_getsid(a[0]),
        sys::PRCTL => proc_ops::sys_prctl(a[0] as u32, a[1], a[2], a[3], a[4]),
        sys::GETRLIMIT => proc_ops::sys_getrlimit(a[0] as u32, a[1]),
        sys::SETRLIMIT => proc_ops::sys_setrlimit(a[0] as u32, a[1]),
        sys::PRLIMIT64 => proc_ops::sys_prlimit64(a[0], a[1] as u32, a[2], a[3]),
        sys::GETRUSAGE => proc_ops::sys_getrusage(a[0] as i32, a[1]),
        sys::UMASK => proc_ops::sys_umask(a[0] as u32),
        sys::UNSHARE => Ok(0),
        sys::SCHED_SETAFFINITY => Ok(0),
        sys::SCHED_GETAFFINITY => proc_ops::sys_sched_getaffinity(a[0], a[1], a[2]),
        sys::SCHED_SETSCHEDULER => Ok(0),
        sys::SCHED_GETSCHEDULER => Ok(0),
        sys::SCHED_GETPARAM => proc_ops::sys_sched_getparam(a[0], a[1]),
        sys::SCHED_GET_PRIORITY_MAX => Ok(0),
        sys::SCHED_GET_PRIORITY_MIN => Ok(0),
        sys::SETPRIORITY | sys::GETPRIORITY => Ok(0),
        sys::RSEQ => Err(crate::err!(ENOSYS)),

        // ---- signals ----
        sys::RT_SIGACTION => signal_ops::sys_rt_sigaction(a[0], a[1], a[2], a[3]),
        sys::RT_SIGPROCMASK => signal_ops::sys_rt_sigprocmask(a[0] as i32, a[1], a[2], a[3]),
        sys::RT_SIGPENDING => signal_ops::sys_rt_sigpending(a[0], a[1]),
        sys::RT_SIGRETURN => return crate::signal::sigreturn(cx),
        sys::RT_SIGSUSPEND => signal_ops::sys_rt_sigsuspend(a[0], a[1]),
        sys::RT_SIGTIMEDWAIT => signal_ops::sys_rt_sigtimedwait(a[0], a[1], a[2], a[3]),
        sys::RT_SIGQUEUEINFO => signal_ops::sys_kill(a[0] as isize, a[1]),
        sys::SIGALTSTACK => signal_ops::sys_sigaltstack(a[0], a[1]),
        sys::KILL => signal_ops::sys_kill(a[0] as isize, a[1]),
        sys::TKILL => signal_ops::sys_tkill(a[0], a[1]),
        sys::TGKILL => signal_ops::sys_tgkill(a[0], a[1], a[2]),

        // ---- polling ----
        sys::PPOLL => poll_ops::sys_ppoll(a[0], a[1], a[2], a[3]),
        sys::PSELECT6 => poll_ops::sys_pselect6(a[0] as i32, a[1], a[2], a[3], a[4], a[5]),
        sys::EPOLL_CREATE1 => poll_ops::sys_epoll_create1(a[0] as u32),
        sys::EPOLL_CTL => poll_ops::sys_epoll_ctl(a[0] as i32, a[1] as u32, a[2] as i32, a[3]),
        sys::EPOLL_PWAIT => {
            poll_ops::sys_epoll_pwait(a[0] as i32, a[1], a[2] as i32, a[3] as i64, a[4])
        }
        sys::EPOLL_PWAIT2 => poll_ops::sys_epoll_pwait2(a[0] as i32, a[1], a[2] as i32, a[3]),
        sys::EVENTFD2 => poll_ops::sys_eventfd2(a[0] as u32, a[1] as u32),
        sys::INOTIFY_INIT1 => Err(crate::err!(ENOSYS)),

        // ---- networking ----
        sys::SOCKET => net_ops::sys_socket(a[0] as u32, a[1] as u32, a[2] as u32),
        sys::SOCKETPAIR => net_ops::sys_socketpair(a[0] as u32, a[1] as u32, a[2] as u32, a[3]),
        sys::BIND => net_ops::sys_bind(a[0] as i32, a[1], a[2]),
        sys::LISTEN => net_ops::sys_listen(a[0] as i32, a[1] as i32),
        sys::ACCEPT => net_ops::sys_accept4(a[0] as i32, a[1], a[2], 0),
        sys::ACCEPT4 => net_ops::sys_accept4(a[0] as i32, a[1], a[2], a[3] as u32),
        sys::CONNECT => net_ops::sys_connect(a[0] as i32, a[1], a[2]),
        sys::GETSOCKNAME => net_ops::sys_getsockname(a[0] as i32, a[1], a[2]),
        sys::GETPEERNAME => net_ops::sys_getpeername(a[0] as i32, a[1], a[2]),
        sys::SENDTO => {
            net_ops::sys_sendto(a[0] as i32, a[1], a[2], a[3] as u32, a[4], a[5])
        }
        sys::RECVFROM => {
            net_ops::sys_recvfrom(a[0] as i32, a[1], a[2], a[3] as u32, a[4], a[5])
        }
        sys::SETSOCKOPT => {
            net_ops::sys_setsockopt(a[0] as i32, a[1] as i32, a[2] as i32, a[3], a[4] as u32)
        }
        sys::GETSOCKOPT => {
            net_ops::sys_getsockopt(a[0] as i32, a[1] as i32, a[2] as i32, a[3], a[4])
        }
        sys::SHUTDOWN => net_ops::sys_shutdown(a[0] as i32, a[1] as i32),
        sys::SENDMSG => net_ops::sys_sendmsg(a[0] as i32, a[1], a[2] as u32),
        sys::RECVMSG => net_ops::sys_recvmsg(a[0] as i32, a[1], a[2] as u32),

        // ---- time and misc ----
        sys::CLOCK_GETTIME => misc_ops::sys_clock_gettime(a[0] as u32, a[1]),
        sys::CLOCK_SETTIME => misc_ops::sys_clock_settime(a[0] as u32, a[1]),
        sys::CLOCK_GETRES => misc_ops::sys_clock_getres(a[0] as u32, a[1]),
        sys::CLOCK_NANOSLEEP => misc_ops::sys_clock_nanosleep(a[0] as u32, a[1] as u32, a[2], a[3]),
        sys::NANOSLEEP => misc_ops::sys_nanosleep(a[0], a[1]),
        sys::GETTIMEOFDAY => misc_ops::sys_gettimeofday(a[0], a[1]),
        sys::SETTIMEOFDAY => Ok(0),
        sys::TIMES => misc_ops::sys_times(a[0]),
        sys::GETITIMER => misc_ops::sys_getitimer(a[0] as u32, a[1]),
        sys::SETITIMER => misc_ops::sys_setitimer(a[0] as u32, a[1], a[2]),
        sys::UNAME => misc_ops::sys_uname(a[0]),
        sys::SYSINFO => misc_ops::sys_sysinfo(a[0]),
        sys::GETRANDOM => misc_ops::sys_getrandom(a[0], a[1], a[2] as u32),
        sys::RISCV_HWPROBE => misc_ops::sys_riscv_hwprobe(a[0], a[1], a[2], a[3], a[4] as u32),
        sys::RISCV_FLUSH_ICACHE => {
            unsafe { core::arch::asm!("fence.i", options(nostack)) };
            Ok(0)
        }

        _ => {
            crate::warn!(
                "unimplemented syscall {} ({}) from pid {} pc={:#x} args=[{:#x} {:#x} {:#x} {:#x} {:#x} {:#x}]",
                n,
                sys::name(n),
                crate::task::current().pid(),
                cx.sepc - 4,
                a[0], a[1], a[2], a[3], a[4], a[5],
            );
            Err(crate::err!(ENOSYS))
        }
    };

    match result {
        Ok(value) => {
            if crate::console::trace_enabled() {
                crate::trace!(
                    "[{}] {}({:#x}, {:#x}, {:#x}) = {}",
                    crate::task::current().pid(),
                    sys::name(n),
                    a[0],
                    a[1],
                    a[2],
                    value
                );
            }
            value
        }
        Err(e) => {
            if crate::console::trace_enabled() {
                crate::trace!(
                    "[{}] {}({:#x}, {:#x}, {:#x}) = -{}",
                    crate::task::current().pid(),
                    sys::name(n),
                    a[0],
                    a[1],
                    a[2],
                    e.errno()
                );
            }
            e.as_ret()
        }
    }
}
