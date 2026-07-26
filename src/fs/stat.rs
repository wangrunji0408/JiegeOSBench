//! Binary layouts shared with user space.

/// `struct timespec`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}

impl Timespec {
    pub fn to_ns(&self) -> u64 {
        (self.sec as u64)
            .wrapping_mul(1_000_000_000)
            .wrapping_add(self.nsec as u64)
    }
}

/// `struct timeval`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Timeval {
    pub sec: i64,
    pub usec: i64,
}

/// The riscv64 `struct stat`, which matches the generic Linux layout.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Kstat {
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
    pub st_atime: Timespec,
    pub st_mtime: Timespec,
    pub st_ctime: Timespec,
    pub __unused: [u32; 2],
}

/// `struct statfs`.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct StatFs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
}

/// `struct linux_dirent64` header; the name follows inline.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Dirent64 {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    // char d_name[];
}

/// `S_IFMT` mask.
pub const S_IFMT: u32 = 0o170000;

/// `struct rlimit`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RLimit {
    pub cur: u64,
    pub max: u64,
}

pub const RLIM_INFINITY: u64 = u64::MAX;

/// `struct sysinfo`, as read by some startup code.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SysInfo {
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
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 0],
}

/// `struct utsname`.
#[repr(C)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl UtsName {
    pub fn new() -> Self {
        fn fill(s: &str) -> [u8; 65] {
            let mut a = [0u8; 65];
            let b = s.as_bytes();
            let n = b.len().min(64);
            a[..n].copy_from_slice(&b[..n]);
            a
        }
        Self {
            // Claim a modern Linux release: musl and nginx both probe the
            // version to decide which syscalls to use, and reporting something
            // recent keeps them on the modern paths we implement.
            sysname: fill("Linux"),
            nodename: fill("jiege"),
            release: fill("6.6.0-jiege"),
            version: fill("#1 SMP jiege-kernel"),
            machine: fill("riscv64"),
            domainname: fill("(none)"),
        }
    }
}

/// `struct tms` for the `times` syscall.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Tms {
    pub utime: i64,
    pub stime: i64,
    pub cutime: i64,
    pub cstime: i64,
}

/// `struct rusage`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RUsage {
    pub utime: Timeval,
    pub stime: Timeval,
    pub maxrss: i64,
    pub ixrss: i64,
    pub idrss: i64,
    pub isrss: i64,
    pub minflt: i64,
    pub majflt: i64,
    pub nswap: i64,
    pub inblock: i64,
    pub oublock: i64,
    pub msgsnd: i64,
    pub msgrcv: i64,
    pub nsignals: i64,
    pub nvcsw: i64,
    pub nivcsw: i64,
}
