//! The VFS inode trait.

use super::stat::Kstat;
use super::{Error, Result};
use crate::bail;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeKind {
    File,
    Dir,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

impl InodeKind {
    /// The `S_IFMT` bits for this kind.
    pub fn mode_bits(self) -> u32 {
        match self {
            InodeKind::Fifo => 0o010000,
            InodeKind::CharDevice => 0o020000,
            InodeKind::Dir => 0o040000,
            InodeKind::BlockDevice => 0o060000,
            InodeKind::File => 0o100000,
            InodeKind::Symlink => 0o120000,
            InodeKind::Socket => 0o140000,
        }
    }

    /// The `DT_*` value used in `getdents64`.
    pub fn dirent_type(self) -> u8 {
        match self {
            InodeKind::Fifo => 1,
            InodeKind::CharDevice => 2,
            InodeKind::Dir => 4,
            InodeKind::BlockDevice => 6,
            InodeKind::File => 8,
            InodeKind::Symlink => 10,
            InodeKind::Socket => 12,
        }
    }
}

pub type InodeRef = Arc<dyn Inode>;

/// One directory entry as returned by [`Inode::readdir`].
pub struct DirEntry {
    pub name: String,
    pub kind: InodeKind,
    pub ino: u64,
}

/// A filesystem object.
///
/// Implementors only need to override the operations that make sense for their
/// kind; the defaults return the appropriate errno.
pub trait Inode: Send + Sync {
    fn kind(&self) -> InodeKind;

    /// Unique inode number.
    fn ino(&self) -> u64;

    /// Size in bytes (0 for devices).
    fn size(&self) -> usize {
        0
    }

    /// Permission bits and setuid/sticky bits (without the type bits).
    fn mode(&self) -> u32 {
        0o644
    }

    fn set_mode(&self, _mode: u32) {}

    /// Owner uid/gid.
    fn owner(&self) -> (u32, u32) {
        (0, 0)
    }

    fn set_owner(&self, _uid: u32, _gid: u32) {}

    /// Read at an absolute offset. Returns the number of bytes read.
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        bail!(EINVAL)
    }

    /// Write at an absolute offset.
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        bail!(EINVAL)
    }

    /// Truncate (or extend with zeros) to `len` bytes.
    fn truncate(&self, _len: usize) -> Result<()> {
        bail!(EINVAL)
    }

    /// Look up a single component in this directory.
    fn lookup(&self, _name: &str) -> Result<InodeRef> {
        bail!(ENOTDIR)
    }

    /// Create a regular file in this directory.
    fn create(&self, _name: &str, _kind: InodeKind, _mode: u32) -> Result<InodeRef> {
        bail!(ENOTDIR)
    }

    /// Link an existing inode into this directory.
    fn link(&self, _name: &str, _inode: &InodeRef) -> Result<()> {
        bail!(EPERM)
    }

    /// Remove a name from this directory.
    fn unlink(&self, _name: &str) -> Result<()> {
        bail!(ENOTDIR)
    }

    /// Rename within/between directories.
    fn rename(&self, _old: &str, _new_dir: &InodeRef, _new: &str) -> Result<()> {
        bail!(EPERM)
    }

    /// List directory entries.
    fn readdir(&self) -> Result<Vec<DirEntry>> {
        bail!(ENOTDIR)
    }

    /// The target of a symlink.
    fn readlink(&self) -> Result<String> {
        bail!(EINVAL)
    }

    /// Create a symlink in this directory.
    fn symlink(&self, _name: &str, _target: &str) -> Result<InodeRef> {
        bail!(EPERM)
    }

    /// Device number for device inodes: (major, minor).
    fn device(&self) -> (u32, u32) {
        (0, 0)
    }

    /// Fill in a `stat` buffer. The default derives everything from the other
    /// methods, which is right for every filesystem we implement.
    fn stat(&self) -> Result<Kstat> {
        let kind = self.kind();
        let (uid, gid) = self.owner();
        let (major, minor) = self.device();
        let mut st = Kstat::default();
        st.st_ino = self.ino();
        st.st_mode = kind.mode_bits() | self.mode();
        st.st_nlink = if kind == InodeKind::Dir { 2 } else { 1 };
        st.st_uid = uid;
        st.st_gid = gid;
        st.st_size = self.size() as i64;
        st.st_blksize = 4096;
        st.st_blocks = ((self.size() + 511) / 512) as i64;
        st.st_dev = 1;
        st.st_rdev = if major != 0 || minor != 0 {
            makedev(major, minor)
        } else {
            0
        };
        let (secs, nsecs) = crate::time::realtime();
        for t in [&mut st.st_atime, &mut st.st_mtime, &mut st.st_ctime] {
            t.sec = secs as i64;
            t.nsec = nsecs as i64;
        }
        Ok(st)
    }

    /// `ioctl`.
    fn ioctl(&self, _cmd: usize, _arg: usize) -> Result<isize> {
        bail!(ENOTTY)
    }

    /// Is data available for reading without blocking?
    fn poll_readable(&self) -> bool {
        true
    }

    /// Can data be written without blocking?
    fn poll_writable(&self) -> bool {
        true
    }

    /// Has the peer hung up / reached EOF? (For pipes and sockets.)
    fn poll_hangup(&self) -> bool {
        false
    }

    /// Is there an error condition pending?
    fn poll_error(&self) -> bool {
        false
    }

    /// Sequential read from a file offset, honoring blocking semantics. Devices
    /// that block (tty, pipe) override this; the default just calls `read_at`.
    fn read(&self, offset: usize, buf: &mut [u8], _nonblock: bool) -> Result<usize> {
        self.read_at(offset, buf)
    }

    fn write(&self, offset: usize, buf: &[u8], _nonblock: bool) -> Result<usize> {
        self.write_at(offset, buf)
    }

    /// Downcast support for sockets, which need their concrete type.
    fn as_any(&self) -> &dyn core::any::Any;
}

pub fn makedev(major: u32, minor: u32) -> u64 {
    // Linux encoding.
    (((major & 0xfff) as u64) << 8)
        | ((minor & 0xff) as u64)
        | (((major >> 12) as u64) << 32)
        | (((minor >> 8) as u64) << 12)
}

/// Allocate a fresh inode number.
pub fn next_ino() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(2);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Helper for inodes that need no downcasting.
#[macro_export]
macro_rules! impl_as_any {
    () => {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
    };
}
