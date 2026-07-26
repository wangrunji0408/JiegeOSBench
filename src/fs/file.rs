//! An open file: an inode plus a cursor and flags.

use super::inode::{Inode, InodeKind, InodeRef};
use super::{Error, Result};
use crate::bail;
use bitflags::bitflags;
use spin::Mutex;

bitflags! {
    /// Linux `O_*` flags (riscv64/generic values).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct OpenFlags: u32 {
        const RDONLY    = 0o0;
        const WRONLY    = 0o1;
        const RDWR      = 0o2;
        const CREAT     = 0o100;
        const EXCL      = 0o200;
        const NOCTTY    = 0o400;
        const TRUNC     = 0o1000;
        const APPEND    = 0o2000;
        const NONBLOCK  = 0o4000;
        const DSYNC     = 0o10000;
        const DIRECT    = 0o40000;
        const LARGEFILE = 0o100000;
        const DIRECTORY = 0o200000;
        const NOFOLLOW  = 0o400000;
        const NOATIME   = 0o1000000;
        const CLOEXEC   = 0o2000000;
        const PATH      = 0o10000000;
        const TMPFILE   = 0o20200000;
    }
}

impl OpenFlags {
    pub fn readable(self) -> bool {
        // The access mode occupies the low two bits.
        let acc = self.bits() & 0o3;
        acc == 0 || acc == 2
    }

    pub fn writable(self) -> bool {
        let acc = self.bits() & 0o3;
        acc == 1 || acc == 2
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SeekFrom {
    Start(i64),
    Current(i64),
    End(i64),
}

/// An open file description. Shared between fds created by `dup` and inherited
/// across `fork`, which is why the cursor is behind a lock.
pub struct File {
    pub inode: InodeRef,
    pub flags: Mutex<OpenFlags>,
    offset: Mutex<usize>,
    /// Directory read position for `getdents64`.
    dir_pos: Mutex<usize>,
    /// The path this was opened by, for `/proc/self/fd` and diagnostics.
    pub path: Mutex<alloc::string::String>,
    /// Incremented whenever a read consumes data. See [`File::read_generation`].
    read_generation: core::sync::atomic::AtomicU64,
}

impl File {
    pub fn new(inode: InodeRef, flags: OpenFlags) -> Self {
        Self {
            inode,
            flags: Mutex::new(flags),
            offset: Mutex::new(0),
            dir_pos: Mutex::new(0),
            path: Mutex::new(alloc::string::String::new()),
            read_generation: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn with_path(inode: InodeRef, flags: OpenFlags, path: &str) -> Self {
        let f = Self::new(inode, flags);
        *f.path.lock() = path.into();
        f
    }

    pub fn is_nonblock(&self) -> bool {
        self.flags.lock().contains(OpenFlags::NONBLOCK)
    }

    pub fn readable(&self) -> bool {
        self.flags.lock().readable()
    }

    pub fn writable(&self) -> bool {
        self.flags.lock().writable()
    }

    pub fn offset(&self) -> usize {
        *self.offset.lock()
    }

    pub fn set_offset(&self, off: usize) {
        *self.offset.lock() = off;
    }

    /// A counter bumped every time a read consumes data from this description.
    ///
    /// Edge-triggered `epoll` uses it to tell a genuinely new arrival from data
    /// that was already reported and never read. Polling alone cannot: between a
    /// reader draining the object and the next byte arriving, nothing observes the
    /// not-ready state in between.
    pub fn read_generation(&self) -> u64 {
        self.read_generation.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Record that data was consumed through a path other than [`File::read`] —
    /// `recvfrom`, `recvmsg`, and `pread` all reach the object directly.
    pub fn note_read(&self, bytes: usize) {
        if bytes > 0 {
            self.read_generation
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Sequential read, advancing the cursor.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize> {
        if !self.readable() {
            bail!(EBADF);
        }
        if self.inode.kind() == InodeKind::Dir {
            bail!(EISDIR);
        }
        let nonblock = self.is_nonblock();
        // Take the offset without holding the lock across a potentially
        // blocking read, or a pipe read would deadlock against the writer's
        // `dup`ed fd.
        let off = *self.offset.lock();
        let n = self.inode.read(off, buf, nonblock)?;
        if n > 0 {
            self.read_generation
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        }
        let mut cursor = self.offset.lock();
        *cursor = off + n;
        Ok(n)
    }

    /// Sequential write, advancing the cursor.
    pub fn write(&self, buf: &[u8]) -> Result<usize> {
        if !self.writable() {
            bail!(EBADF);
        }
        let nonblock = self.is_nonblock();
        let append = self.flags.lock().contains(OpenFlags::APPEND);
        let off = if append {
            self.inode.size()
        } else {
            *self.offset.lock()
        };
        let n = self.inode.write(off, buf, nonblock)?;
        let mut cursor = self.offset.lock();
        *cursor = off + n;
        Ok(n)
    }

    /// Positional read that doesn't touch the cursor (`pread`, and the page
    /// fault handler for file-backed mappings).
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.inode.read_at(offset, buf)
    }

    pub fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        self.inode.write_at(offset, buf)
    }

    pub fn seek(&self, pos: SeekFrom) -> Result<usize> {
        if matches!(
            self.inode.kind(),
            InodeKind::Fifo | InodeKind::Socket | InodeKind::CharDevice
        ) {
            // Character devices are seekable in Linux only for some drivers;
            // /dev/null and friends accept it, pipes and sockets do not.
            if matches!(self.inode.kind(), InodeKind::Fifo | InodeKind::Socket) {
                bail!(ESPIPE);
            }
        }
        let mut cursor = self.offset.lock();
        let new = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(n) => *cursor as i64 + n,
            SeekFrom::End(n) => self.inode.size() as i64 + n,
        };
        if new < 0 {
            bail!(EINVAL);
        }
        *cursor = new as usize;
        Ok(new as usize)
    }

    pub fn dir_pos(&self) -> usize {
        *self.dir_pos.lock()
    }

    pub fn set_dir_pos(&self, pos: usize) {
        *self.dir_pos.lock() = pos;
    }

    /// Duplicate the description for `fork`. `dup` shares the same `Arc`, so it
    /// does not go through here.
    pub fn poll_readable(&self) -> bool {
        self.inode.poll_readable()
    }

    pub fn poll_writable(&self) -> bool {
        self.inode.poll_writable()
    }

    pub fn poll_hangup(&self) -> bool {
        self.inode.poll_hangup()
    }

    pub fn poll_rdhup(&self) -> bool {
        self.inode.poll_rdhup()
    }

    pub fn poll_error(&self) -> bool {
        self.inode.poll_error()
    }

    /// Get the socket behind this file, if it is one.
    pub fn as_socket(&self) -> Option<&crate::net::socket::Socket> {
        self.inode.as_any().downcast_ref::<crate::net::socket::Socket>()
    }
}

impl core::fmt::Debug for File {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "File(ino={}, kind={:?}, off={})",
            self.inode.ino(),
            self.inode.kind(),
            self.offset()
        )
    }
}

impl Error {
    /// Is this the "would block" error?
    pub fn is_again(self) -> bool {
        self.0 == super::errno::EAGAIN
    }
}
