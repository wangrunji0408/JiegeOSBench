//! Syscall dispatch. Numbers follow the generic riscv64/arm64 Linux ABI
//! (`include/uapi/asm-generic/unistd.h`), confirmed against a real strace
//! of nginx running under `qemu-riscv64 -strace`.

mod fs;
mod mm;
mod misc;
mod net;
mod poll;
mod process;

const SYSCALL_GETCWD: usize = 17;
const SYSCALL_EVENTFD2: usize = 19;
const SYSCALL_EPOLL_CREATE1: usize = 20;
const SYSCALL_EPOLL_CTL: usize = 21;
const SYSCALL_EPOLL_PWAIT: usize = 22;
const SYSCALL_DUP3: usize = 24;
const SYSCALL_FCNTL: usize = 25;
const SYSCALL_IOCTL: usize = 29;
const SYSCALL_MKDIRAT: usize = 34;
const SYSCALL_UNLINKAT: usize = 35;
const SYSCALL_UMOUNT2: usize = 39;
const SYSCALL_STATFS: usize = 43;
const SYSCALL_FACCESSAT: usize = 48;
const SYSCALL_OPENAT: usize = 56;
const SYSCALL_CLOSE: usize = 57;
const SYSCALL_PIPE2: usize = 59;
const SYSCALL_LSEEK: usize = 62;
const SYSCALL_READ: usize = 63;
const SYSCALL_WRITE: usize = 64;
const SYSCALL_READV: usize = 65;
const SYSCALL_WRITEV: usize = 66;
const SYSCALL_PREAD64: usize = 67;
const SYSCALL_PWRITE64: usize = 68;
const SYSCALL_READLINKAT: usize = 78;
const SYSCALL_NEWFSTATAT: usize = 79;
const SYSCALL_FSTAT: usize = 80;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;
const SYSCALL_SET_TID_ADDRESS: usize = 96;
const SYSCALL_FUTEX: usize = 98;
const SYSCALL_NANOSLEEP: usize = 101;
const SYSCALL_SETITIMER: usize = 103;
const SYSCALL_CLOCK_GETTIME: usize = 113;
const SYSCALL_SCHED_SETAFFINITY: usize = 122;
const SYSCALL_SCHED_GETAFFINITY: usize = 123;
const SYSCALL_SCHED_YIELD: usize = 124;
const SYSCALL_KILL: usize = 129;
const SYSCALL_TGKILL: usize = 131;
const SYSCALL_RT_SIGSUSPEND: usize = 133;
const SYSCALL_RT_SIGACTION: usize = 134;
const SYSCALL_RT_SIGPROCMASK: usize = 135;
const SYSCALL_RT_SIGRETURN: usize = 139;
const SYSCALL_SETGID: usize = 144;
const SYSCALL_SETUID: usize = 146;
const SYSCALL_SETGROUPS: usize = 159;
const SYSCALL_UNAME: usize = 160;
const SYSCALL_GETRLIMIT: usize = 163;
const SYSCALL_SETRLIMIT: usize = 164;
const SYSCALL_PRCTL: usize = 167;
const SYSCALL_GETTIMEOFDAY: usize = 169;
const SYSCALL_GETPID: usize = 172;
const SYSCALL_GETPPID: usize = 173;
const SYSCALL_GETUID: usize = 174;
const SYSCALL_GETEUID: usize = 175;
const SYSCALL_GETGID: usize = 176;
const SYSCALL_GETEGID: usize = 177;
const SYSCALL_GETTID: usize = 178;
const SYSCALL_SOCKET: usize = 198;
const SYSCALL_SOCKETPAIR: usize = 199;
const SYSCALL_BIND: usize = 200;
const SYSCALL_LISTEN: usize = 201;
const SYSCALL_ACCEPT: usize = 202;
const SYSCALL_CONNECT: usize = 203;
const SYSCALL_GETSOCKNAME: usize = 204;
const SYSCALL_GETPEERNAME: usize = 205;
const SYSCALL_SENDTO: usize = 206;
const SYSCALL_RECVFROM: usize = 207;
const SYSCALL_SETSOCKOPT: usize = 208;
const SYSCALL_GETSOCKOPT: usize = 209;
const SYSCALL_SHUTDOWN: usize = 210;
const SYSCALL_SENDMSG: usize = 211;
const SYSCALL_RECVMSG: usize = 212;
const SYSCALL_BRK: usize = 214;
const SYSCALL_MUNMAP: usize = 215;
const SYSCALL_CLONE: usize = 220;
const SYSCALL_EXECVE: usize = 221;
const SYSCALL_MMAP: usize = 222;
const SYSCALL_MPROTECT: usize = 226;
const SYSCALL_MADVISE: usize = 233;
const SYSCALL_ACCEPT4: usize = 242;
const SYSCALL_WAIT4: usize = 260;
const SYSCALL_PRLIMIT64: usize = 261;
const SYSCALL_GETRANDOM: usize = 278;
const SYSCALL_IO_SETUP: usize = 0;
const SYSCALL_IO_DESTROY: usize = 1;

pub fn syscall(id: usize, args: [usize; 6]) -> isize {
    match id {
        SYSCALL_WRITE => fs::sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_READ => fs::sys_read(args[0], args[1] as *mut u8, args[2]),
        SYSCALL_PREAD64 => fs::sys_pread64(args[0], args[1] as *mut u8, args[2], args[3]),
        SYSCALL_PWRITE64 => fs::sys_pwrite64(args[0], args[1] as *const u8, args[2], args[3]),
        SYSCALL_CLOSE => fs::sys_close(args[0]),
        SYSCALL_DUP3 => fs::sys_dup3(args[0], args[1], args[2]),
        SYSCALL_OPENAT => fs::sys_openat(args[0] as isize, args[1] as *const u8, args[2] as u32, args[3] as u32),
        SYSCALL_MKDIRAT => fs::sys_mkdirat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_UNLINKAT => fs::sys_unlinkat(args[0] as isize, args[1] as *const u8, args[2] as u32),
        SYSCALL_LSEEK => fs::sys_lseek(args[0], args[1] as isize, args[2]),
        SYSCALL_FSTAT => fs::sys_fstat(args[0], args[1] as *mut u8),
        SYSCALL_NEWFSTATAT => fs::sys_newfstatat(args[0] as isize, args[1] as *const u8, args[2] as *mut u8, args[3] as u32),
        SYSCALL_FACCESSAT => fs::sys_faccessat(args[0] as isize, args[1] as *const u8),
        SYSCALL_READLINKAT => -38, // ENOSYS: nothing in our target workload follows symlinks explicitly
        SYSCALL_GETCWD => fs::sys_getcwd(args[0] as *mut u8, args[1]),
        SYSCALL_STATFS => -38,
        SYSCALL_UMOUNT2 => 0,
        SYSCALL_READV => fs::sys_readv(args[0], args[1] as *const u8, args[2]),
        SYSCALL_WRITEV => fs::sys_writev(args[0], args[1] as *const u8, args[2]),
        SYSCALL_PIPE2 => fs::sys_pipe2(args[0] as *mut u8),

        SYSCALL_BRK => mm::sys_brk(args[0]),
        SYSCALL_MMAP => mm::sys_mmap(args[0], args[1], args[2], args[3], args[4] as isize, args[5]),
        SYSCALL_MUNMAP => mm::sys_munmap(args[0], args[1]),
        SYSCALL_MPROTECT => mm::sys_mprotect(args[0], args[1], args[2]),
        SYSCALL_MADVISE => 0,

        SYSCALL_EXIT | SYSCALL_EXIT_GROUP => process::sys_exit(args[0] as i32),
        SYSCALL_CLONE => process::sys_clone(args[0], args[1], args[2], args[3], args[4]),
        SYSCALL_EXECVE => process::sys_execve(args[0] as *const u8, args[1] as *const usize, args[2] as *const usize),
        SYSCALL_WAIT4 => process::sys_wait4(args[0] as isize, args[1] as *mut i32, args[2] as u32),
        SYSCALL_SET_TID_ADDRESS => process::sys_getpid_like(),
        SYSCALL_SCHED_YIELD => process::sys_sched_yield(),
        SYSCALL_NANOSLEEP => process::sys_nanosleep(args[0] as *const u8),
        SYSCALL_KILL => process::sys_kill(args[0] as isize, args[1] as i32),
        SYSCALL_TGKILL => process::sys_kill(args[0] as isize, args[2] as i32),
        SYSCALL_FUTEX => 0,

        SYSCALL_GETPID => misc::sys_getpid(),
        SYSCALL_GETPPID => misc::sys_getppid(),
        SYSCALL_GETTID => misc::sys_gettid(),
        SYSCALL_GETUID | SYSCALL_GETEUID | SYSCALL_GETGID | SYSCALL_GETEGID => misc::sys_getuid(),
        SYSCALL_SETUID | SYSCALL_SETGID => misc::sys_setuid_like(args[0]),
        SYSCALL_SETGROUPS => misc::sys_setgroups(args[0], args[1]),
        SYSCALL_PRCTL => misc::sys_prctl(args[0], args[1]),
        SYSCALL_UNAME => misc::sys_uname(args[0] as *mut u8),
        SYSCALL_CLOCK_GETTIME => misc::sys_clock_gettime(args[0], args[1] as *mut u8),
        SYSCALL_GETTIMEOFDAY => misc::sys_gettimeofday(args[0] as *mut u8),
        SYSCALL_SCHED_GETAFFINITY => misc::sys_sched_getaffinity(args[0], args[1], args[2] as *mut u8),
        SYSCALL_SCHED_SETAFFINITY => 0,
        SYSCALL_PRLIMIT64 => misc::sys_prlimit64(args[0], args[1], args[2], args[3]),
        SYSCALL_GETRLIMIT | SYSCALL_SETRLIMIT => 0,
        SYSCALL_GETRANDOM => misc::sys_getrandom(args[0] as *mut u8, args[1], args[2]),
        SYSCALL_SETITIMER => misc::sys_setitimer(),
        SYSCALL_IOCTL => misc::sys_ioctl(args[0], args[1], args[2]),
        SYSCALL_FCNTL => misc::sys_fcntl(args[0], args[1], args[2]),
        SYSCALL_RT_SIGACTION | SYSCALL_RT_SIGPROCMASK | SYSCALL_RT_SIGRETURN | SYSCALL_RT_SIGSUSPEND => {
            misc::sys_rt_sig_stub()
        }
        SYSCALL_IO_SETUP | SYSCALL_IO_DESTROY => -38, // ENOSYS: nginx disables AIO gracefully on this

        SYSCALL_SOCKET | SYSCALL_SOCKETPAIR | SYSCALL_BIND | SYSCALL_LISTEN | SYSCALL_ACCEPT
        | SYSCALL_ACCEPT4 | SYSCALL_CONNECT | SYSCALL_GETSOCKNAME | SYSCALL_GETPEERNAME
        | SYSCALL_SENDTO | SYSCALL_RECVFROM | SYSCALL_SETSOCKOPT | SYSCALL_GETSOCKOPT
        | SYSCALL_SHUTDOWN | SYSCALL_SENDMSG | SYSCALL_RECVMSG | SYSCALL_EVENTFD2
        | SYSCALL_EPOLL_CREATE1 | SYSCALL_EPOLL_CTL | SYSCALL_EPOLL_PWAIT => {
            crate::println!("[kernel] net/epoll syscall id={} not yet implemented", id);
            -38
        }

        _ => {
            crate::println!("[kernel] unsupported syscall id={}, args={:?}", id, args);
            -38 // ENOSYS
        }
    }
}
