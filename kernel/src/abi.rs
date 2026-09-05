//! Linux riscv64 ABI constants and structures.
#![allow(dead_code)]

// ---- errno ----
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const E2BIG: i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const EFBIG: i32 = 27;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EMLINK: i32 = 31;
pub const EPIPE: i32 = 32;
pub const ERANGE: i32 = 34;
pub const EDEADLK: i32 = 35;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;
pub const ELOOP: i32 = 40;
pub const ENOMSG: i32 = 42;
pub const EOVERFLOW: i32 = 75;
pub const ENOTSOCK: i32 = 88;
pub const EDESTADDRREQ: i32 = 89;
pub const EMSGSIZE: i32 = 90;
pub const EPROTOTYPE: i32 = 91;
pub const ENOPROTOOPT: i32 = 92;
pub const EPROTONOSUPPORT: i32 = 93;
pub const EOPNOTSUPP: i32 = 95;
pub const EAFNOSUPPORT: i32 = 97;
pub const EADDRINUSE: i32 = 98;
pub const EADDRNOTAVAIL: i32 = 99;
pub const ENETDOWN: i32 = 100;
pub const ENETUNREACH: i32 = 101;
pub const ECONNABORTED: i32 = 103;
pub const ECONNRESET: i32 = 104;
pub const ENOBUFS: i32 = 105;
pub const EISCONN: i32 = 106;
pub const ENOTCONN: i32 = 107;
pub const ETIMEDOUT: i32 = 110;
pub const ECONNREFUSED: i32 = 111;
pub const EHOSTUNREACH: i32 = 113;
pub const EALREADY: i32 = 114;
pub const EINPROGRESS: i32 = 115;
/// Internal: restart the syscall (never returned to user).
pub const ERESTART: i32 = 512;

pub type SysResult = Result<usize, i32>;

// ---- open flags ----
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_ACCMODE: u32 = 3;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_NOCTTY: u32 = 0o400;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DSYNC: u32 = 0o10000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_NOFOLLOW: u32 = 0o400000;
pub const O_CLOEXEC: u32 = 0o2000000;
pub const O_PATH: u32 = 0o10000000;
pub const O_LARGEFILE: u32 = 0o100000;
pub const O_DIRECT: u32 = 0o40000;
pub const O_NOATIME: u32 = 0o1000000;
pub const O_TMPFILE: u32 = 0o20000000 | O_DIRECTORY;

pub const AT_FDCWD: i32 = -100;
pub const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
pub const AT_REMOVEDIR: u32 = 0x200;
pub const AT_SYMLINK_FOLLOW: u32 = 0x400;
pub const AT_EMPTY_PATH: u32 = 0x1000;

pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

// ---- file mode ----
pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;

/// Kernel `struct stat` for riscv64 (asm-generic), 128 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad1: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub __pad2: i32,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: u64,
    pub st_mtime: i64,
    pub st_mtime_nsec: u64,
    pub st_ctime: i64,
    pub st_ctime_nsec: u64,
    pub __unused: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: u16,
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub __spare3: [u64; 12],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Iovec {
    pub base: usize,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Rlimit {
    pub cur: u64,
    pub max: u64,
}

pub const RLIMIT_CPU: u32 = 0;
pub const RLIMIT_FSIZE: u32 = 1;
pub const RLIMIT_DATA: u32 = 2;
pub const RLIMIT_STACK: u32 = 3;
pub const RLIMIT_CORE: u32 = 4;
pub const RLIMIT_RSS: u32 = 5;
pub const RLIMIT_NPROC: u32 = 6;
pub const RLIMIT_NOFILE: u32 = 7;
pub const RLIMIT_MEMLOCK: u32 = 8;
pub const RLIMIT_AS: u32 = 9;
pub const RLIM_INFINITY: u64 = !0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl Default for Utsname {
    fn default() -> Self {
        Utsname {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        }
    }
}

// ---- mmap ----
pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;
pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;
pub const MAP_NORESERVE: u32 = 0x4000;
pub const MAP_STACK: u32 = 0x20000;
pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;

// ---- signals ----
pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGQUIT: i32 = 3;
pub const SIGILL: i32 = 4;
pub const SIGTRAP: i32 = 5;
pub const SIGABRT: i32 = 6;
pub const SIGBUS: i32 = 7;
pub const SIGFPE: i32 = 8;
pub const SIGKILL: i32 = 9;
pub const SIGUSR1: i32 = 10;
pub const SIGSEGV: i32 = 11;
pub const SIGUSR2: i32 = 12;
pub const SIGPIPE: i32 = 13;
pub const SIGALRM: i32 = 14;
pub const SIGTERM: i32 = 15;
pub const SIGSTKFLT: i32 = 16;
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGSTOP: i32 = 19;
pub const SIGTSTP: i32 = 20;
pub const SIGTTIN: i32 = 21;
pub const SIGTTOU: i32 = 22;
pub const SIGURG: i32 = 23;
pub const SIGXCPU: i32 = 24;
pub const SIGXFSZ: i32 = 25;
pub const SIGVTALRM: i32 = 26;
pub const SIGPROF: i32 = 27;
pub const SIGWINCH: i32 = 28;
pub const SIGIO: i32 = 29;
pub const SIGPWR: i32 = 30;
pub const SIGSYS: i32 = 31;
pub const NSIG: usize = 65;

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

pub const SA_NOCLDSTOP: u64 = 1;
pub const SA_NOCLDWAIT: u64 = 2;
pub const SA_SIGINFO: u64 = 4;
pub const SA_RESTORER: u64 = 0x04000000;
pub const SA_ONSTACK: u64 = 0x08000000;
pub const SA_RESTART: u64 = 0x10000000;
pub const SA_NODEFER: u64 = 0x40000000;
pub const SA_RESETHAND: u64 = 0x80000000;

pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// Kernel sigaction (riscv64: no restorer).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct KSigAction {
    pub handler: usize,
    pub flags: u64,
    pub mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct StackT {
    pub ss_sp: usize,
    pub ss_flags: i32,
    pub _pad: i32,
    pub ss_size: usize,
}
pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;

/// siginfo_t (128 bytes). We only fill the common fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigInfo {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code: i32,
    pub _pad0: i32,
    // union: for SIGCHLD: pid, uid, status; for SIGSEGV: addr
    pub si_pid: i32,
    pub si_uid: u32,
    pub si_status: i32,
    pub _pad1: i32,
    pub _rest: [u64; 12],
}

impl Default for SigInfo {
    fn default() -> Self {
        SigInfo {
            si_signo: 0,
            si_errno: 0,
            si_code: 0,
            _pad0: 0,
            si_pid: 0,
            si_uid: 0,
            si_status: 0,
            _pad1: 0,
            _rest: [0; 12],
        }
    }
}

pub const SI_USER: i32 = 0;
pub const SI_KERNEL: i32 = 0x80;
pub const CLD_EXITED: i32 = 1;
pub const CLD_KILLED: i32 = 2;

/// struct ucontext for riscv64.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UContext {
    pub uc_flags: u64,
    pub uc_link: usize,
    pub uc_stack: StackT,
    pub uc_sigmask: u64,
    pub _unused: [u8; 128],
    // struct sigcontext at offset 176 (16-aligned): 32 gregs (pc, ra, sp, ...), then fp state
    pub sc_regs: [usize; 32],
    pub sc_fpregs: [u64; 32],
    pub sc_fcsr: u32,
    pub _fpad: u32,
    pub _fres: [u64; 33], // remaining of the 528-byte fp union + reserved
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RtSigFrame {
    pub info: SigInfo,
    pub uc: UContext,
}

// ---- wait ----
pub const WNOHANG: i32 = 1;
pub const WUNTRACED: i32 = 2;
pub const WEXITED: i32 = 4;
pub const WCONTINUED: i32 = 8;
pub const WNOWAIT: i32 = 0x01000000;
pub const P_ALL: i32 = 0;
pub const P_PID: i32 = 1;
pub const P_PGID: i32 = 2;

// ---- clone ----
pub const CLONE_VM: u64 = 0x100;
pub const CLONE_FS: u64 = 0x200;
pub const CLONE_FILES: u64 = 0x400;
pub const CLONE_SIGHAND: u64 = 0x800;
pub const CLONE_VFORK: u64 = 0x4000;
pub const CLONE_PARENT: u64 = 0x8000;
pub const CLONE_THREAD: u64 = 0x10000;
pub const CLONE_SYSVSEM: u64 = 0x40000;
pub const CLONE_SETTLS: u64 = 0x80000;
pub const CLONE_PARENT_SETTID: u64 = 0x100000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x200000;
pub const CLONE_CHILD_SETTID: u64 = 0x1000000;

// ---- fcntl ----
pub const F_DUPFD: u32 = 0;
pub const F_GETFD: u32 = 1;
pub const F_SETFD: u32 = 2;
pub const F_GETFL: u32 = 3;
pub const F_SETFL: u32 = 4;
pub const F_GETLK: u32 = 5;
pub const F_SETLK: u32 = 6;
pub const F_SETLKW: u32 = 7;
pub const F_SETOWN: u32 = 8;
pub const F_GETOWN: u32 = 9;
pub const F_DUPFD_CLOEXEC: u32 = 1030;
pub const FD_CLOEXEC: u32 = 1;

// ---- ioctl ----
pub const TCGETS: u32 = 0x5401;
pub const TCSETS: u32 = 0x5402;
pub const TCSETSW: u32 = 0x5403;
pub const TCSETSF: u32 = 0x5404;
pub const TIOCGPGRP: u32 = 0x540F;
pub const TIOCSPGRP: u32 = 0x5410;
pub const TIOCGWINSZ: u32 = 0x5413;
pub const TIOCSWINSZ: u32 = 0x5414;
pub const FIONREAD: u32 = 0x541B;
pub const FIONBIO: u32 = 0x5421;
pub const FIOASYNC: u32 = 0x5452;
pub const FIOCLEX: u32 = 0x5451;
pub const FIONCLEX: u32 = 0x5450;
pub const TIOCSCTTY: u32 = 0x540E;
pub const TIOCGSID: u32 = 0x5429;

/// struct termios (kernel, 36 bytes without c_ispeed/c_ospeed = 44 with).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; 19],
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

// ---- poll/epoll ----
pub const POLLIN: u32 = 0x001;
pub const POLLPRI: u32 = 0x002;
pub const POLLOUT: u32 = 0x004;
pub const POLLERR: u32 = 0x008;
pub const POLLHUP: u32 = 0x010;
pub const POLLNVAL: u32 = 0x020;
pub const POLLRDNORM: u32 = 0x040;
pub const POLLRDBAND: u32 = 0x080;
pub const POLLWRNORM: u32 = 0x100;
pub const POLLWRBAND: u32 = 0x200;
pub const POLLRDHUP: u32 = 0x2000;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLEXCLUSIVE: u32 = 1 << 28;
pub const EPOLLWAKEUP: u32 = 1 << 29;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;
pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;
pub const EPOLL_CLOEXEC: u32 = O_CLOEXEC;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct EpollEvent {
    pub events: u32,
    pub _pad: u32,
    pub data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

// ---- sockets ----
pub const AF_UNSPEC: u16 = 0;
pub const AF_UNIX: u16 = 1;
pub const AF_INET: u16 = 2;
pub const AF_INET6: u16 = 10;
pub const AF_NETLINK: u16 = 16;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;
pub const SOCK_TYPE_MASK: u32 = 0xf;
pub const SOCK_NONBLOCK: u32 = O_NONBLOCK;
pub const SOCK_CLOEXEC: u32 = O_CLOEXEC;

pub const SOL_SOCKET: i32 = 1;
pub const SOL_IP: i32 = 0;
pub const SOL_TCP: i32 = 6;
pub const SOL_IPV6: i32 = 41;
pub const SO_DEBUG: i32 = 1;
pub const SO_REUSEADDR: i32 = 2;
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_DONTROUTE: i32 = 5;
pub const SO_BROADCAST: i32 = 6;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVBUF: i32 = 8;
pub const SO_KEEPALIVE: i32 = 9;
pub const SO_OOBINLINE: i32 = 10;
pub const SO_LINGER: i32 = 13;
pub const SO_REUSEPORT: i32 = 15;
pub const SO_RCVLOWAT: i32 = 18;
pub const SO_SNDLOWAT: i32 = 19;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_ACCEPTCONN: i32 = 30;
pub const SO_PROTOCOL: i32 = 38;
pub const SO_DOMAIN: i32 = 39;
pub const TCP_NODELAY: i32 = 1;
pub const TCP_MAXSEG: i32 = 2;
pub const TCP_CORK: i32 = 3;
pub const TCP_KEEPIDLE: i32 = 4;
pub const TCP_KEEPINTVL: i32 = 5;
pub const TCP_KEEPCNT: i32 = 6;
pub const TCP_DEFER_ACCEPT: i32 = 9;
pub const TCP_INFO: i32 = 11;
pub const TCP_QUICKACK: i32 = 12;
pub const TCP_FASTOPEN: i32 = 23;

pub const MSG_OOB: u32 = 1;
pub const MSG_PEEK: u32 = 2;
pub const MSG_DONTROUTE: u32 = 4;
pub const MSG_CTRUNC: u32 = 8;
pub const MSG_TRUNC: u32 = 0x20;
pub const MSG_DONTWAIT: u32 = 0x40;
pub const MSG_EOR: u32 = 0x80;
pub const MSG_WAITALL: u32 = 0x100;
pub const MSG_NOSIGNAL: u32 = 0x4000;
pub const MSG_CMSG_CLOEXEC: u32 = 0x40000000;

pub const SHUT_RD: i32 = 0;
pub const SHUT_WR: i32 = 1;
pub const SHUT_RDWR: i32 = 2;

pub const SCM_RIGHTS: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16, // network byte order
    pub sin_addr: u32, // network byte order
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct MsgHdr {
    pub msg_name: usize,
    pub msg_namelen: u32,
    pub _pad0: u32,
    pub msg_iov: usize,
    pub msg_iovlen: usize,
    pub msg_control: usize,
    pub msg_controllen: usize,
    pub msg_flags: i32,
    pub _pad1: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct CmsgHdr {
    pub cmsg_len: usize,
    pub cmsg_level: i32,
    pub cmsg_type: i32,
}

// ---- futex ----
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

// ---- clocks ----
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
pub const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
pub const CLOCK_MONOTONIC_RAW: i32 = 4;
pub const CLOCK_REALTIME_COARSE: i32 = 5;
pub const CLOCK_MONOTONIC_COARSE: i32 = 6;
pub const CLOCK_BOOTTIME: i32 = 7;

// ---- auxv ----
pub const AT_NULL: usize = 0;
pub const AT_IGNORE: usize = 1;
pub const AT_EXECFD: usize = 2;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_FLAGS: usize = 8;
pub const AT_ENTRY: usize = 9;
pub const AT_NOTELF: usize = 10;
pub const AT_UID: usize = 11;
pub const AT_EUID: usize = 12;
pub const AT_GID: usize = 13;
pub const AT_EGID: usize = 14;
pub const AT_PLATFORM: usize = 15;
pub const AT_HWCAP: usize = 16;
pub const AT_CLKTCK: usize = 17;
pub const AT_SECURE: usize = 23;
pub const AT_BASE_PLATFORM: usize = 24;
pub const AT_RANDOM: usize = 25;
pub const AT_HWCAP2: usize = 26;
pub const AT_EXECFN: usize = 31;
pub const AT_SYSINFO_EHDR: usize = 33;

// ---- misc ----
pub const PR_SET_NAME: i32 = 15;
pub const PR_GET_NAME: i32 = 16;
pub const PR_SET_DUMPABLE: i32 = 4;
pub const PR_GET_DUMPABLE: i32 = 3;

pub const EFD_SEMAPHORE: u32 = 1;
pub const EFD_CLOEXEC: u32 = O_CLOEXEC;
pub const EFD_NONBLOCK: u32 = O_NONBLOCK;

pub const MADV_NORMAL: i32 = 0;
pub const MADV_DONTNEED: i32 = 4;
pub const MADV_FREE: i32 = 8;

/// struct linux_dirent64 header (name follows).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Dirent64Hdr {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Sysinfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub _pad2: u32,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Rusage {
    pub ru_utime: Timeval,
    pub ru_stime: Timeval,
    pub rest: [i64; 14],
}

// ---- syscall numbers (asm-generic / riscv64) ----
pub mod nr {
    pub const IO_SETUP: usize = 0;
    pub const IO_DESTROY: usize = 1;
    pub const IO_SUBMIT: usize = 2;
    pub const IO_GETEVENTS: usize = 4;
    pub const SETXATTR: usize = 5;
    pub const GETXATTR: usize = 8;
    pub const LGETXATTR: usize = 9;
    pub const FGETXATTR: usize = 10;
    pub const LISTXATTR: usize = 11;
    pub const GETCWD: usize = 17;
    pub const EVENTFD2: usize = 19;
    pub const EPOLL_CREATE1: usize = 20;
    pub const EPOLL_CTL: usize = 21;
    pub const EPOLL_PWAIT: usize = 22;
    pub const DUP: usize = 23;
    pub const DUP3: usize = 24;
    pub const FCNTL: usize = 25;
    pub const INOTIFY_INIT1: usize = 26;
    pub const IOCTL: usize = 29;
    pub const IOPRIO_SET: usize = 30;
    pub const FLOCK: usize = 32;
    pub const MKNODAT: usize = 33;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
    pub const SYMLINKAT: usize = 36;
    pub const LINKAT: usize = 37;
    pub const UMOUNT2: usize = 39;
    pub const MOUNT: usize = 40;
    pub const STATFS: usize = 43;
    pub const FSTATFS: usize = 44;
    pub const TRUNCATE: usize = 45;
    pub const FTRUNCATE: usize = 46;
    pub const FALLOCATE: usize = 47;
    pub const FACCESSAT: usize = 48;
    pub const CHDIR: usize = 49;
    pub const FCHDIR: usize = 50;
    pub const CHROOT: usize = 51;
    pub const FCHMOD: usize = 52;
    pub const FCHMODAT: usize = 53;
    pub const FCHOWNAT: usize = 54;
    pub const FCHOWN: usize = 55;
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const VHANGUP: usize = 58;
    pub const PIPE2: usize = 59;
    pub const GETDENTS64: usize = 61;
    pub const LSEEK: usize = 62;
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const READV: usize = 65;
    pub const WRITEV: usize = 66;
    pub const PREAD64: usize = 67;
    pub const PWRITE64: usize = 68;
    pub const PREADV: usize = 69;
    pub const PWRITEV: usize = 70;
    pub const SENDFILE: usize = 71;
    pub const PSELECT6: usize = 72;
    pub const PPOLL: usize = 73;
    pub const SIGNALFD4: usize = 74;
    pub const SPLICE: usize = 76;
    pub const READLINKAT: usize = 78;
    pub const NEWFSTATAT: usize = 79;
    pub const FSTAT: usize = 80;
    pub const SYNC: usize = 81;
    pub const FSYNC: usize = 82;
    pub const FDATASYNC: usize = 83;
    pub const TIMERFD_CREATE: usize = 85;
    pub const TIMERFD_SETTIME: usize = 86;
    pub const TIMERFD_GETTIME: usize = 87;
    pub const UTIMENSAT: usize = 88;
    pub const ACCT: usize = 89;
    pub const CAPGET: usize = 90;
    pub const CAPSET: usize = 91;
    pub const PERSONALITY: usize = 92;
    pub const EXIT: usize = 93;
    pub const EXIT_GROUP: usize = 94;
    pub const WAITID: usize = 95;
    pub const SET_TID_ADDRESS: usize = 96;
    pub const UNSHARE: usize = 97;
    pub const FUTEX: usize = 98;
    pub const SET_ROBUST_LIST: usize = 99;
    pub const GET_ROBUST_LIST: usize = 100;
    pub const NANOSLEEP: usize = 101;
    pub const GETITIMER: usize = 102;
    pub const SETITIMER: usize = 103;
    pub const TIMER_CREATE: usize = 107;
    pub const TIMER_SETTIME: usize = 110;
    pub const TIMER_DELETE: usize = 111;
    pub const CLOCK_SETTIME: usize = 112;
    pub const CLOCK_GETTIME: usize = 113;
    pub const CLOCK_GETRES: usize = 114;
    pub const CLOCK_NANOSLEEP: usize = 115;
    pub const SYSLOG: usize = 116;
    pub const PTRACE: usize = 117;
    pub const SCHED_SETPARAM: usize = 118;
    pub const SCHED_SETSCHEDULER: usize = 119;
    pub const SCHED_GETSCHEDULER: usize = 120;
    pub const SCHED_GETPARAM: usize = 121;
    pub const SCHED_SETAFFINITY: usize = 122;
    pub const SCHED_GETAFFINITY: usize = 123;
    pub const SCHED_YIELD: usize = 124;
    pub const SCHED_GET_PRIORITY_MAX: usize = 125;
    pub const SCHED_GET_PRIORITY_MIN: usize = 126;
    pub const KILL: usize = 129;
    pub const TKILL: usize = 130;
    pub const TGKILL: usize = 131;
    pub const SIGALTSTACK: usize = 132;
    pub const RT_SIGSUSPEND: usize = 133;
    pub const RT_SIGACTION: usize = 134;
    pub const RT_SIGPROCMASK: usize = 135;
    pub const RT_SIGPENDING: usize = 136;
    pub const RT_SIGTIMEDWAIT: usize = 137;
    pub const RT_SIGQUEUEINFO: usize = 138;
    pub const RT_SIGRETURN: usize = 139;
    pub const SETPRIORITY: usize = 140;
    pub const GETPRIORITY: usize = 141;
    pub const REBOOT: usize = 142;
    pub const SETREGID: usize = 143;
    pub const SETGID: usize = 144;
    pub const SETREUID: usize = 145;
    pub const SETUID: usize = 146;
    pub const SETRESUID: usize = 147;
    pub const GETRESUID: usize = 148;
    pub const SETRESGID: usize = 149;
    pub const GETRESGID: usize = 150;
    pub const SETFSUID: usize = 151;
    pub const SETFSGID: usize = 152;
    pub const TIMES: usize = 153;
    pub const SETPGID: usize = 154;
    pub const GETPGID: usize = 155;
    pub const GETSID: usize = 156;
    pub const SETSID: usize = 157;
    pub const GETGROUPS: usize = 158;
    pub const SETGROUPS: usize = 159;
    pub const UNAME: usize = 160;
    pub const SETHOSTNAME: usize = 161;
    pub const SETDOMAINNAME: usize = 162;
    pub const GETRLIMIT: usize = 163;
    pub const SETRLIMIT: usize = 164;
    pub const GETRUSAGE: usize = 165;
    pub const UMASK: usize = 166;
    pub const PRCTL: usize = 167;
    pub const GETCPU: usize = 168;
    pub const GETTIMEOFDAY: usize = 169;
    pub const SETTIMEOFDAY: usize = 170;
    pub const ADJTIMEX: usize = 171;
    pub const GETPID: usize = 172;
    pub const GETPPID: usize = 173;
    pub const GETUID: usize = 174;
    pub const GETEUID: usize = 175;
    pub const GETGID: usize = 176;
    pub const GETEGID: usize = 177;
    pub const GETTID: usize = 178;
    pub const SYSINFO: usize = 179;
    pub const MQ_OPEN: usize = 180;
    pub const MSGGET: usize = 186;
    pub const SEMGET: usize = 190;
    pub const SHMGET: usize = 194;
    pub const SHMCTL: usize = 195;
    pub const SHMAT: usize = 196;
    pub const SHMDT: usize = 197;
    pub const SOCKET: usize = 198;
    pub const SOCKETPAIR: usize = 199;
    pub const BIND: usize = 200;
    pub const LISTEN: usize = 201;
    pub const ACCEPT: usize = 202;
    pub const CONNECT: usize = 203;
    pub const GETSOCKNAME: usize = 204;
    pub const GETPEERNAME: usize = 205;
    pub const SENDTO: usize = 206;
    pub const RECVFROM: usize = 207;
    pub const SETSOCKOPT: usize = 208;
    pub const GETSOCKOPT: usize = 209;
    pub const SHUTDOWN: usize = 210;
    pub const SENDMSG: usize = 211;
    pub const RECVMSG: usize = 212;
    pub const READAHEAD: usize = 213;
    pub const BRK: usize = 214;
    pub const MUNMAP: usize = 215;
    pub const MREMAP: usize = 216;
    pub const ADD_KEY: usize = 217;
    pub const CLONE: usize = 220;
    pub const EXECVE: usize = 221;
    pub const MMAP: usize = 222;
    pub const FADVISE64: usize = 223;
    pub const SWAPON: usize = 224;
    pub const MPROTECT: usize = 226;
    pub const MSYNC: usize = 227;
    pub const MLOCK: usize = 228;
    pub const MUNLOCK: usize = 229;
    pub const MLOCKALL: usize = 230;
    pub const MUNLOCKALL: usize = 231;
    pub const MINCORE: usize = 232;
    pub const MADVISE: usize = 233;
    pub const REMAP_FILE_PAGES: usize = 234;
    pub const MBIND: usize = 235;
    pub const GET_MEMPOLICY: usize = 236;
    pub const SET_MEMPOLICY: usize = 237;
    pub const RT_TGSIGQUEUEINFO: usize = 240;
    pub const PERF_EVENT_OPEN: usize = 241;
    pub const ACCEPT4: usize = 242;
    pub const RECVMMSG: usize = 243;
    pub const WAIT4: usize = 260;
    pub const PRLIMIT64: usize = 261;
    pub const FANOTIFY_INIT: usize = 262;
    pub const NAME_TO_HANDLE_AT: usize = 264;
    pub const CLOCK_ADJTIME: usize = 266;
    pub const SYNCFS: usize = 267;
    pub const SETNS: usize = 268;
    pub const SENDMMSG: usize = 269;
    pub const PROCESS_VM_READV: usize = 270;
    pub const KCMP: usize = 272;
    pub const FINIT_MODULE: usize = 273;
    pub const SCHED_SETATTR: usize = 274;
    pub const SCHED_GETATTR: usize = 275;
    pub const RENAMEAT2: usize = 276;
    pub const SECCOMP: usize = 277;
    pub const GETRANDOM: usize = 278;
    pub const MEMFD_CREATE: usize = 279;
    pub const BPF: usize = 280;
    pub const EXECVEAT: usize = 281;
    pub const USERFAULTFD: usize = 282;
    pub const MEMBARRIER: usize = 283;
    pub const MLOCK2: usize = 284;
    pub const COPY_FILE_RANGE: usize = 285;
    pub const PREADV2: usize = 286;
    pub const PWRITEV2: usize = 287;
    pub const PKEY_MPROTECT: usize = 288;
    pub const STATX: usize = 291;
    pub const IO_PGETEVENTS: usize = 292;
    pub const RSEQ: usize = 293;
    pub const KEXEC_FILE_LOAD: usize = 294;
    pub const PIDFD_SEND_SIGNAL: usize = 424;
    pub const IO_URING_SETUP: usize = 425;
    pub const IO_URING_ENTER: usize = 426;
    pub const IO_URING_REGISTER: usize = 427;
    pub const OPEN_TREE: usize = 428;
    pub const MOVE_MOUNT: usize = 429;
    pub const FSOPEN: usize = 430;
    pub const PIDFD_OPEN: usize = 434;
    pub const CLONE3: usize = 435;
    pub const CLOSE_RANGE: usize = 436;
    pub const OPENAT2: usize = 437;
    pub const PIDFD_GETFD: usize = 438;
    pub const FACCESSAT2: usize = 439;
    pub const PROCESS_MADVISE: usize = 440;
    pub const EPOLL_PWAIT2: usize = 441;
    pub const MOUNT_SETATTR: usize = 442;
    pub const LANDLOCK_CREATE_RULESET: usize = 444;
    pub const FUTEX_WAITV: usize = 449;
    pub const SET_MEMPOLICY_HOME_NODE: usize = 450;
    pub const CACHESTAT: usize = 451;
    pub const FCHMODAT2: usize = 452;
    pub const MAP_SHADOW_STACK: usize = 453;
    pub const FUTEX_WAKE: usize = 454;
    pub const FUTEX_WAIT: usize = 455;
    pub const FUTEX_REQUEUE: usize = 456;
    pub const STATMOUNT: usize = 457;
    pub const LISTMOUNT: usize = 458;
    pub const LSM_GET_SELF_ATTR: usize = 459;
    pub const MSEAL: usize = 462;
    pub const RISCV_HWPROBE: usize = 258;
    pub const RISCV_FLUSH_ICACHE: usize = 259;
}
