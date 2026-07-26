//! Filesystem layer: VFS inodes, a writable ramfs, device files, and pipes.

pub mod device;
pub mod file;
pub mod fdtable;
pub mod inode;
pub mod path;
pub mod pipe;
pub mod procfs;
pub mod ramfs;
pub mod stat;
pub mod tar;

pub use fdtable::FdTable;
pub use file::{File, OpenFlags, SeekFrom};
pub use inode::{Inode, InodeKind, InodeRef};
pub use stat::{Kstat, StatFs};

use alloc::sync::Arc;
use spin::Once;

static ROOT: Once<InodeRef> = Once::new();

/// The root directory inode.
pub fn root() -> &'static InodeRef {
    ROOT.get().expect("filesystem not mounted")
}

/// Mount the root filesystem, unpacking the embedded rootfs archive into it.
pub fn init(archive: &'static [u8]) {
    let root = ramfs::RamDir::new_root();
    ROOT.call_once(|| root.clone());

    // Standard directories that the archive may not contain.
    for dir in [
        "/dev", "/proc", "/sys", "/tmp", "/run", "/var", "/var/tmp", "/etc", "/lib", "/usr",
        "/usr/lib", "/usr/sbin", "/usr/share",
    ] {
        let _ = path::mkdir_p(dir, 0o755);
    }

    let count = tar::extract(archive).expect("failed to unpack rootfs archive");
    crate::info!("rootfs: unpacked {} entries ({} KiB)", count, archive.len() / 1024);

    device::init();
    procfs::init();
}

/// Errno values we return from the filesystem and syscall layers.
pub mod errno {
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
    pub const EXDEV: isize = 18;
    pub const ENODEV: isize = 19;
    pub const ENOTDIR: isize = 20;
    pub const EISDIR: isize = 21;
    pub const EINVAL: isize = 22;
    pub const ENFILE: isize = 23;
    pub const EMFILE: isize = 24;
    pub const ENOTTY: isize = 25;
    pub const ETXTBSY: isize = 26;
    pub const EFBIG: isize = 27;
    pub const ENOSPC: isize = 28;
    pub const ESPIPE: isize = 29;
    pub const EROFS: isize = 30;
    pub const EMLINK: isize = 31;
    pub const EPIPE: isize = 32;
    pub const EDOM: isize = 33;
    pub const ERANGE: isize = 34;
    pub const ENAMETOOLONG: isize = 36;
    pub const ENOSYS: isize = 38;
    pub const ENOTEMPTY: isize = 39;
    pub const ELOOP: isize = 40;
    pub const ENOTSOCK: isize = 88;
    pub const EMSGSIZE: isize = 90;
    pub const EPROTONOSUPPORT: isize = 93;
    pub const EAFNOSUPPORT: isize = 97;
    pub const EADDRINUSE: isize = 98;
    pub const EADDRNOTAVAIL: isize = 99;
    pub const ENETDOWN: isize = 100;
    pub const ENETUNREACH: isize = 101;
    pub const ECONNABORTED: isize = 103;
    pub const ECONNRESET: isize = 104;
    pub const ENOBUFS: isize = 105;
    pub const EISCONN: isize = 106;
    pub const ENOTCONN: isize = 107;
    pub const ETIMEDOUT: isize = 110;
    pub const ECONNREFUSED: isize = 111;
    pub const EHOSTUNREACH: isize = 113;
    pub const EALREADY: isize = 114;
    pub const EINPROGRESS: isize = 115;
    pub const ENOTSUP: isize = 95;
    pub const EOPNOTSUPP: isize = 95;
}

/// Kernel-internal error type; converts to a negative errno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub isize);

impl Error {
    pub const fn new(errno: isize) -> Self {
        Self(errno)
    }
    pub const fn errno(self) -> isize {
        self.0
    }
    /// The value to return to user space.
    pub const fn as_ret(self) -> isize {
        -self.0
    }
}

pub type Result<T> = core::result::Result<T, Error>;

impl From<crate::mm::uaccess::Fault> for Error {
    fn from(_: crate::mm::uaccess::Fault) -> Self {
        Error(errno::EFAULT)
    }
}

/// Shorthand for constructing errors: `err!(ENOENT)`.
#[macro_export]
macro_rules! err {
    ($name:ident) => {
        $crate::fs::Error::new($crate::fs::errno::$name)
    };
}

/// Shorthand for returning errors: `bail!(ENOENT)`.
#[macro_export]
macro_rules! bail {
    ($name:ident) => {
        return Err($crate::err!($name))
    };
}

/// Convenience: wrap an inode in a `File` with the given flags.
pub fn open_file(inode: InodeRef, flags: OpenFlags) -> Arc<File> {
    Arc::new(File::new(inode, flags))
}
