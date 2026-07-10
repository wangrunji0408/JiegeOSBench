mod fs;
mod mm;
mod net;
mod process;
mod time;

use crate::arch::context::TrapContext;

// Linux riscv64 syscall numbers
const SYS_IO_SETUP: usize = 0;
const SYS_READ: usize = 63;
const SYS_WRITE: usize = 64;
const SYS_READV: usize = 65;
const SYS_WRITEV: usize = 66;
const SYS_PREAD64: usize = 67;
const SYS_PWRITE64: usize = 68;
const SYS_SENDFILE: usize = 71;
const SYS_SPLICE: usize = 76;
const SYS_OPENAT: usize = 56;
const SYS_CLOSE: usize = 57;
const SYS_LSEEK: usize = 62;
const SYS_IOCTL: usize = 29;
const SYS_FCNTL: usize = 25;
const SYS_FSTAT: usize = 80;
const SYS_NEWFSTATAT: usize = 79;
const SYS_MKDIRAT: usize = 34;
const SYS_UNLINKAT: usize = 35;
const SYS_RENAMEAT: usize = 38;
const SYS_RENAMEAT2: usize = 276;
const SYS_GETDENTS64: usize = 61;
const SYS_CHDIR: usize = 49;
const SYS_GETCWD: usize = 17;
const SYS_FACCESSAT: usize = 48;
const SYS_FACCESSAT2: usize = 439;
const SYS_ACCESS: usize = usize::MAX; // 不存在于riscv64
const SYS_PIPE2: usize = 59;
const SYS_DUP: usize = 23;
const SYS_DUP3: usize = 24;
const SYS_SOCKET: usize = 198;
const SYS_BIND: usize = 200;
const SYS_LISTEN: usize = 201;
const SYS_ACCEPT: usize = 202;
const SYS_ACCEPT4: usize = 242;
const SYS_CONNECT: usize = 203;
const SYS_GETSOCKNAME: usize = 204;
const SYS_GETPEERNAME: usize = 205;
const SYS_SETSOCKOPT: usize = 208;
const SYS_GETSOCKOPT: usize = 209;
const SYS_SENDTO: usize = 206;
const SYS_RECVFROM: usize = 207;
const SYS_SENDMSG: usize = 211;
const SYS_RECVMSG: usize = 212;
const SYS_SHUTDOWN: usize = 210;
const SYS_SOCKETPAIR: usize = 199;
const SYS_CLONE: usize = 220;
const SYS_EXECVE: usize = 221;
const SYS_EXIT: usize = 93;
const SYS_EXIT_GROUP: usize = 94;
const SYS_WAIT4: usize = 260;
const SYS_GETPID: usize = 172;
const SYS_GETPPID: usize = 173;
const SYS_GETUID: usize = 174;
const SYS_GETEUID: usize = 175;
const SYS_GETGID: usize = 176;
const SYS_GETEGID: usize = 177;
const SYS_GETTID: usize = 178;
const SYS_MMAP: usize = 222;
const SYS_MUNMAP: usize = 215;
const SYS_MPROTECT: usize = 226;
const SYS_MREMAP: usize = 216;
const SYS_BRK: usize = 214;
const SYS_MADVISE: usize = 233;
const SYS_NANOSLEEP: usize = 101;
const SYS_CLOCK_GETTIME: usize = 113;
const SYS_GETTIMEOFDAY: usize = 169;
const SYS_TIMES: usize = 153;
const SYS_UNAME: usize = 160;
const SYS_GETRLIMIT: usize = 163;
const SYS_SETRLIMIT: usize = 164;
const SYS_PRLIMIT64: usize = 261;
const SYS_SIGPROCMASK: usize = 135;
const SYS_SIGACTION: usize = 134;
const SYS_SIGSUSPEND: usize = 133;
const SYS_RT_SIGPROCMASK: usize = 135;
const SYS_RT_SIGACTION: usize = 134;
const SYS_RT_SIGRETURN: usize = 139;
const SYS_KILL: usize = 129;
const SYS_TGKILL: usize = 131;
const SYS_SETPGID: usize = 154;
const SYS_GETPGID: usize = 155;
const SYS_SETSID: usize = 157;
const SYS_UMASK: usize = 166;
const SYS_STATFS: usize = 43;
const SYS_FSTATFS: usize = 44;
const SYS_EVENTFD2: usize = 19;
const SYS_EPOLL_CREATE1: usize = 20;
const SYS_EPOLL_CTL: usize = 21;
const SYS_EPOLL_WAIT: usize = 22; // 不存在，用EPOLL_PWAIT
const SYS_EPOLL_PWAIT: usize = 22;
const SYS_POLL: usize = 73;
const SYS_PPOLL: usize = 73;
const SYS_SELECT: usize = usize::MAX;
const SYS_PSELECT6: usize = 72;
const SYS_GETRUSAGE: usize = 165;
const SYS_SYSINFO: usize = 179;
const SYS_PRCTL: usize = 167;
const SYS_ARCH_PRCTL: usize = usize::MAX;
const SYS_FUTEX: usize = 98;
const SYS_SET_ROBUST_LIST: usize = 99;
const SYS_GET_ROBUST_LIST: usize = 100;
const SYS_SETITIMER: usize = 103;
const SYS_GETITIMER: usize = 102;
const SYS_SIGALTSTACK: usize = 132;
const SYS_CAPGET: usize = 90;
const SYS_CAPSET: usize = 91;
const SYS_SCHED_GETAFFINITY: usize = 123;
const SYS_SCHED_SETAFFINITY: usize = 122;
const SYS_SCHED_YIELD: usize = 124;
const SYS_SCHED_GETSCHEDULER: usize = 120;
const SYS_SCHED_SETSCHEDULER: usize = 119;
const SYS_SCHED_GETPARAM: usize = 121;
const SYS_SET_TID_ADDRESS: usize = 96;
const SYS_GETRANDOM: usize = 278;
const SYS_RISCV_HWPROBE: usize = 258;
const SYS_RISCV_FLUSH_ICACHE: usize = 259;
const SYS_MEMBARRIER: usize = 283;
const SYS_RSEQ: usize = 293;
const SYS_SEMTIMEDOP: usize = 192;
const SYS_SEMOP: usize = 193;
const SYS_SHMGET: usize = 194;
const SYS_SHMCTL: usize = 195;
const SYS_SHMAT: usize = 196;
const SYS_SHMDT: usize = 197;
const SYS_COPY_FILE_RANGE: usize = 285;
const SYS_INOTIFY_INIT1: usize = 26;
const SYS_INOTIFY_ADD_WATCH: usize = 27;
const SYS_INOTIFY_RM_WATCH: usize = 28;
const SYS_TIMERFD_CREATE: usize = 85;
const SYS_TIMERFD_SETTIME: usize = 86;
const SYS_TIMERFD_GETTIME: usize = 87;
const SYS_SIGNALFD4: usize = 74;
const SYS_RECVMMSG: usize = 243;
const SYS_SENDMMSG: usize = 269;
const SYS_TRUNCATE: usize = 45;
// User/group ID syscalls - return success (we run as root)
const SYS_SETUID: usize = 146;
const SYS_SETGID: usize = 144;
const SYS_SETGROUPS: usize = 159;
const SYS_SETRESUID: usize = 147;
const SYS_SETRESGID: usize = 149;
const SYS_SETREUID: usize = 145;
const SYS_SETREGID: usize = 143;
const SYS_FTRUNCATE: usize = 46;
const SYS_FALLOCATE: usize = 47;
const SYS_CHMOD: usize = usize::MAX;
const SYS_FCHMOD: usize = 52;
const SYS_FCHMODAT: usize = 53;
const SYS_CHOWN: usize = usize::MAX;
const SYS_FCHOWN: usize = 55;
const SYS_FCHOWNAT: usize = 54;
const SYS_SYMLINKAT: usize = 36;
const SYS_READLINKAT: usize = 78;
const SYS_LINKAT: usize = 37;
const SYS_UTIMENSAT: usize = 88;

// errno
pub const EPERM: isize = -1;
pub const ENOENT: isize = -2;
pub const ESRCH: isize = -3;
pub const EINTR: isize = -4;
pub const EIO: isize = -5;
pub const ENXIO: isize = -6;
pub const E2BIG: isize = -7;
pub const ENOEXEC: isize = -8;
pub const EBADF: isize = -9;
pub const ECHILD: isize = -10;
pub const EAGAIN: isize = -11;
pub const EWOULDBLOCK: isize = -11;
pub const ENOMEM: isize = -12;
pub const EACCES: isize = -13;
pub const EFAULT: isize = -14;
pub const EBUSY: isize = -16;
pub const EEXIST: isize = -17;
pub const EXDEV: isize = -18;
pub const ENODEV: isize = -19;
pub const ENOTDIR: isize = -20;
pub const EISDIR: isize = -21;
pub const EINVAL: isize = -22;
pub const EMFILE: isize = -24;
pub const ENFILE: isize = -23;
pub const EFBIG: isize = -27;
pub const ENOSPC: isize = -28;
pub const ESPIPE: isize = -29;
pub const EROFS: isize = -30;
pub const EPIPE: isize = -32;
pub const ERANGE: isize = -34;
pub const ENAMETOOLONG: isize = -36;
pub const ENOSYS: isize = -38;
pub const ENOTEMPTY: isize = -39;
pub const EADDRINUSE: isize = -98;
pub const EADDRNOTAVAIL: isize = -99;
pub const ENETDOWN: isize = -100;
pub const ENETUNREACH: isize = -101;
pub const ECONNRESET: isize = -104;
pub const ENOBUFS: isize = -105;
pub const EISCONN: isize = -106;
pub const ENOTCONN: isize = -107;
pub const ETIMEDOUT: isize = -110;
pub const ECONNREFUSED: isize = -111;
pub const EHOSTUNREACH: isize = -113;
pub const EALREADY: isize = -114;
pub const EINPROGRESS: isize = -115;

pub fn syscall(id: usize, args: [usize; 6], cx: &mut TrapContext) -> isize {
    match id {
        SYS_READ => fs::sys_read(args[0] as i32, args[1], args[2]),
        SYS_WRITE => fs::sys_write(args[0] as i32, args[1], args[2]),
        SYS_READV => fs::sys_readv(args[0] as i32, args[1], args[2] as i32),
        SYS_WRITEV => fs::sys_writev(args[0] as i32, args[1], args[2] as i32),
        SYS_PREAD64 => fs::sys_pread64(args[0] as i32, args[1], args[2], args[3] as i64),
        SYS_PWRITE64 => {
            // pwrite64: 在指定偏移写入，不改变文件位置
            let old_off = {
                let task = crate::task::current_task().unwrap();
                let t = task.lock();
                match t.fds.get(&(args[0] as i32)) {
                    Some(crate::task::process::FileDesc::File { offset, .. }) => *offset,
                    _ => 0,
                }
            };
            // 设置临时偏移
            {
                let task = crate::task::current_task().unwrap();
                let mut t = task.lock();
                if let Some(crate::task::process::FileDesc::File { offset, .. }) = t.fds.get_mut(&(args[0] as i32)) {
                    *offset = args[3];
                }
            }
            let n = fs::sys_write(args[0] as i32, args[1], args[2]);
            // 恢复偏移
            {
                let task = crate::task::current_task().unwrap();
                let mut t = task.lock();
                if let Some(crate::task::process::FileDesc::File { offset, .. }) = t.fds.get_mut(&(args[0] as i32)) {
                    *offset = old_off;
                }
            }
            n
        }
        SYS_OPENAT => fs::sys_openat(args[0] as i32, args[1], args[2] as i32, args[3] as u32),
        SYS_CLOSE => fs::sys_close(args[0] as i32),
        SYS_LSEEK => fs::sys_lseek(args[0] as i32, args[1] as i64, args[2] as i32),
        SYS_IOCTL => fs::sys_ioctl(args[0] as i32, args[1], args[2]),
        SYS_FCNTL => fs::sys_fcntl(args[0] as i32, args[1] as i32, args[2]),
        SYS_FSTAT => fs::sys_fstat(args[0] as i32, args[1]),
        SYS_NEWFSTATAT => fs::sys_newfstatat(args[0] as i32, args[1], args[2], args[3] as i32),
        SYS_MKDIRAT => fs::sys_mkdirat(args[0] as i32, args[1], args[2] as u32),
        SYS_UNLINKAT => fs::sys_unlinkat(args[0] as i32, args[1], args[2] as i32),
        SYS_GETDENTS64 => fs::sys_getdents64(args[0] as i32, args[1], args[2]),
        SYS_CHDIR => fs::sys_chdir(args[0]),
        SYS_GETCWD => fs::sys_getcwd(args[0], args[1]),
        SYS_FACCESSAT => fs::sys_faccessat(args[0] as i32, args[1], args[2] as i32, args[3] as i32),
        SYS_FACCESSAT2 => fs::sys_faccessat(args[0] as i32, args[1], args[2] as i32, args[3] as i32),
        SYS_PIPE2 => fs::sys_pipe2(args[0], args[1] as i32),
        SYS_DUP => fs::sys_dup(args[0] as i32),
        SYS_DUP3 => fs::sys_dup3(args[0] as i32, args[1] as i32, args[2] as i32),
        SYS_READLINKAT => fs::sys_readlinkat(args[0] as i32, args[1], args[2], args[3]),
        SYS_TRUNCATE => fs::sys_truncate(args[0], args[1] as i64),
        SYS_FTRUNCATE => fs::sys_ftruncate(args[0] as i32, args[1] as i64),
        SYS_UTIMENSAT => fs::sys_utimensat(args[0] as i32, args[1], args[2], args[3] as i32),
        SYS_STATFS => fs::sys_statfs(args[0], args[1]),
        SYS_FSTATFS => fs::sys_fstatfs(args[0] as i32, args[1]),
        SYS_SENDFILE => fs::sys_sendfile(args[0] as i32, args[1] as i32, args[2], args[3]),
        SYS_SYMLINKAT => fs::sys_symlinkat(args[0], args[1] as i32, args[2]),
        SYS_FCHMOD | SYS_FCHMODAT => 0,
        SYS_FCHOWN | SYS_FCHOWNAT => 0, // 忽略chown
        SYS_LINKAT => 0,

        SYS_SOCKET => net::sys_socket(args[0] as i32, args[1] as i32, args[2] as i32),
        SYS_BIND => net::sys_bind(args[0] as i32, args[1], args[2] as u32),
        SYS_LISTEN => net::sys_listen(args[0] as i32, args[1] as i32),
        SYS_ACCEPT => net::sys_accept(args[0] as i32, args[1], args[2]),
        SYS_ACCEPT4 => net::sys_accept4(args[0] as i32, args[1], args[2], args[3] as i32),
        SYS_CONNECT => net::sys_connect(args[0] as i32, args[1], args[2] as u32),
        SYS_GETSOCKNAME => net::sys_getsockname(args[0] as i32, args[1], args[2]),
        SYS_GETPEERNAME => net::sys_getpeername(args[0] as i32, args[1], args[2]),
        SYS_SETSOCKOPT => net::sys_setsockopt(args[0] as i32, args[1] as i32, args[2] as i32, args[3], args[4] as u32),
        SYS_GETSOCKOPT => net::sys_getsockopt(args[0] as i32, args[1] as i32, args[2] as i32, args[3], args[4]),
        SYS_SENDTO => net::sys_sendto(args[0] as i32, args[1], args[2], args[3] as i32, args[4], args[5] as u32),
        SYS_RECVFROM => net::sys_recvfrom(args[0] as i32, args[1], args[2], args[3] as i32, args[4], args[5]),
        SYS_SENDMSG => net::sys_sendmsg(args[0] as i32, args[1], args[2] as i32),
        SYS_RECVMSG => net::sys_recvmsg(args[0] as i32, args[1], args[2] as i32),
        SYS_SHUTDOWN => net::sys_shutdown(args[0] as i32, args[1] as i32),
        SYS_SOCKETPAIR => net::sys_socketpair(args[0] as i32, args[1] as i32, args[2] as i32, args[3]),

        SYS_CLONE => process::sys_clone(args[0], args[1], args[2], args[3], args[4], cx),
        SYS_EXECVE => process::sys_execve(args[0], args[1], args[2], cx),
        SYS_EXIT => process::sys_exit(args[0] as i32),
        SYS_EXIT_GROUP => process::sys_exit(args[0] as i32),
        SYS_WAIT4 => process::sys_wait4(args[0] as i32, args[1], args[2] as i32),
        SYS_GETPID => process::sys_getpid(),
        SYS_GETPPID => process::sys_getppid(),
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => 0,
        SYS_GETTID => process::sys_getpid(), // 简化：tid=pid
        SYS_UNAME => process::sys_uname(args[0]),
        SYS_GETRLIMIT => process::sys_getrlimit(args[0] as i32, args[1]),
        SYS_SETRLIMIT => 0, // 忽略
        SYS_PRLIMIT64 => process::sys_prlimit64(args[0] as i32, args[1] as i32, args[2], args[3]),
        SYS_KILL => process::sys_kill(args[0] as i32, args[1] as i32),
        SYS_TGKILL => process::sys_kill(args[1] as i32, args[2] as i32),
        SYS_SETPGID => 0,
        SYS_GETPGID => process::sys_getpid(),
        SYS_SETSID => process::sys_getpid(),
        SYS_UMASK => 0o022,
        SYS_PRCTL => 0,
        SYS_GETRUSAGE => process::sys_getrusage(args[0] as i32, args[1]),
        SYS_SYSINFO => process::sys_sysinfo(args[0]),
        SYS_SET_TID_ADDRESS => process::sys_getpid(),
        SYS_CAPGET | SYS_CAPSET => 0,
        SYS_SCHED_YIELD => { crate::task::schedule(); 0 }
        SYS_SCHED_GETAFFINITY => process::sys_sched_getaffinity(args[0] as i32, args[1], args[2]),
        SYS_SCHED_SETAFFINITY => 0,
        SYS_SCHED_GETSCHEDULER => 0, // SCHED_OTHER
        SYS_SCHED_SETSCHEDULER => 0,
        SYS_SCHED_GETPARAM => process::sys_sched_getparam(args[0] as i32, args[1]),
        // User/group ID management - return success (we pretend to run as root/nobody)
        SYS_SETUID | SYS_SETGID | SYS_SETGROUPS |
        SYS_SETRESUID | SYS_SETRESGID | SYS_SETREUID | SYS_SETREGID => 0,

        SYS_MMAP => mm::sys_mmap(args[0], args[1], args[2] as i32, args[3] as i32, args[4] as i32, args[5] as i64),
        SYS_MUNMAP => mm::sys_munmap(args[0], args[1]),
        SYS_MPROTECT => {
            // Just return success, no patching for now
            0
        }, // 忽略权限变更
        SYS_MADVISE => 0,
        SYS_BRK => mm::sys_brk(args[0]),
        SYS_MREMAP => mm::sys_mremap(args[0], args[1], args[2], args[3] as i32, args[4]),

        SYS_NANOSLEEP => time::sys_nanosleep(args[0], args[1]),
        SYS_CLOCK_GETTIME => time::sys_clock_gettime(args[0] as i32, args[1]),
        SYS_GETTIMEOFDAY => time::sys_gettimeofday(args[0], args[1]),
        SYS_TIMES => time::sys_times(args[0]),

        SYS_RT_SIGPROCMASK => process::sys_rt_sigprocmask(args[0] as i32, args[1], args[2], args[3]),
        SYS_RT_SIGACTION => process::sys_rt_sigaction(args[0] as i32, args[1], args[2], args[3]),
        SYS_RT_SIGRETURN => 0,
        SYS_SIGALTSTACK => 0,
        SYS_SIGSUSPEND => {
            // rt_sigsuspend: wait for a signal
            // Implementation: block for a short time (polls network), then return EINTR
            // nginx will use this in its event loop to wait for incoming connections
            crate::net::poll();
            // Wait a bit (poll timer)
            let wait_until = crate::timer::get_time_ms() + 100; // 100ms wait
            while crate::timer::get_time_ms() < wait_until {
                // Yield to other tasks if any
                crate::task::schedule();
                crate::net::poll();
            }
            EINTR // Return EINTR to simulate signal arrival
        }

        SYS_FUTEX => process::sys_futex(args[0], args[1] as i32, args[2] as u32, args[3], args[4], args[5] as u32),
        SYS_SET_ROBUST_LIST => 0,
        SYS_GET_ROBUST_LIST => ENOSYS,

        SYS_EPOLL_CREATE1 => net::sys_epoll_create1(args[0] as i32),
        SYS_EPOLL_CTL => net::sys_epoll_ctl(args[0] as i32, args[1] as i32, args[2] as i32, args[3]),
        SYS_EPOLL_PWAIT => net::sys_epoll_pwait(args[0] as i32, args[1], args[2] as i32, args[3] as i32, args[4]),
        SYS_POLL | SYS_PPOLL => net::sys_poll(args[0], args[1] as u32, args[2] as i32),
        SYS_PSELECT6 => net::sys_pselect6(args[0] as i32, args[1], args[2], args[3], args[4], args[5]),
        SYS_INOTIFY_INIT1 => net::sys_inotify_init1(args[0] as i32),
        SYS_INOTIFY_ADD_WATCH => ENOSYS,
        SYS_INOTIFY_RM_WATCH => ENOSYS,
        SYS_TIMERFD_CREATE => net::sys_timerfd_create(args[0] as i32, args[1] as i32),
        SYS_TIMERFD_SETTIME => 0,
        SYS_TIMERFD_GETTIME => 0,
        SYS_EVENTFD2 => net::sys_eventfd2(args[0] as u32, args[1] as i32),
        SYS_RECVMMSG => net::sys_recvmmsg(args[0] as i32, args[1], args[2] as u32, args[3] as i32, args[4]),
        SYS_SENDMMSG => net::sys_sendmmsg(args[0] as i32, args[1], args[2] as u32, args[3] as i32),

        SYS_GETRANDOM => {
            // 生成随机数（使用时间戳作为伪随机种子）
            let buf_va = args[0];
            let buflen = args[1];
            let flags = args[2];
            let task = crate::task::current_task().unwrap();
            let t = task.lock();
            let mut buf = alloc::vec![0u8; buflen];
            let time = crate::timer::get_time_us();
            for (i, b) in buf.iter_mut().enumerate() {
                *b = ((time >> (i % 8)) ^ (i as usize).wrapping_mul(0x9e3779b9)) as u8;
            }
            t.memory_set.copy_to_user(buf_va, &buf);
            buflen as isize
        }
        SYS_RISCV_HWPROBE => {
            // riscv_hwprobe: 探测RISC-V硬件特性
            // pairs: struct riscv_hwprobe { key, value }
            // 返回0=成功，设置各特性的value
            let pairs_va = args[0];
            let pair_count = args[1];
            let cpu_count = args[2]; // 0 = all CPUs
            let task = crate::task::current_task().unwrap();
            let t = task.lock();
            // 我们返回基本的RISC-V特性
            for i in 0..pair_count {
                let pair_va = pairs_va + i * 16; // sizeof(riscv_hwprobe) = 16
                let mut pair = [0u8; 16];
                t.memory_set.copy_from_user(pair_va, &mut pair);
                let key = i64::from_le_bytes(pair[0..8].try_into().unwrap());
                let value: i64 = match key {
                    1 => 0,      // RISCV_HWPROBE_KEY_MVENDORID
                    2 => 0,      // RISCV_HWPROBE_KEY_MARCHID
                    3 => 0,      // RISCV_HWPROBE_KEY_MIMPID
                    4 => 1,      // RISCV_HWPROBE_KEY_BASE_BEHAVIOR: 只报告IMA基本特性
                    5 => 0,      // RISCV_HWPROBE_KEY_IMA_EXT_0: 无扩展
                    6 => 0,      // RISCV_HWPROBE_KEY_CPUPERF_0
                    _ => -1,     // unknown key
                };
                pair[8..16].copy_from_slice(&value.to_le_bytes());
                t.memory_set.copy_to_user(pair_va, &pair);
            }
            0
        }
        SYS_RISCV_FLUSH_ICACHE => 0,
        SYS_RSEQ => {
            // Restartable Sequences - 简化实现：初始化rseq结构体并返回成功
            // struct rseq { cpu_id_start: i32, cpu_id: i32, rseq_cs: u64, flags: u32 }
            // glibc用这个实现快速用户空间路径
            // 我们只需要把cpu_id设置为合理值（0）
            let rseq_va = args[0];
            let rseq_len = args[1];
            let flags = args[2] as i32;
            // 初始化rseq结构体（cpu_id_start=0, cpu_id=0, rseq_cs=0, flags=0）
            if rseq_va != 0 && rseq_len >= 20 {
                let task = crate::task::current_task().unwrap();
                let t = task.lock();
                let zeros = [0u8; 20];
                t.memory_set.copy_to_user(rseq_va, &zeros);
            }
            0
        }
        SYS_MEMBARRIER => 0,
        SYS_SHMGET => process::sys_shmget(args[0], args[1], args[2] as i32),
        SYS_SHMCTL => 0,
        SYS_SHMAT => process::sys_shmat(args[0] as i32, args[1], args[2] as i32),
        SYS_SHMDT => 0,
        SYS_SEMOP | SYS_SEMTIMEDOP => 0,

        SYS_MEMFD_CREATE => ENOSYS,

        _ => {
            println!("[syscall] Unknown syscall id={} (a7={:#x})", id, id);
            ENOSYS
        }
    }
}
