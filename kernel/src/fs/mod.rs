//! File abstraction shared by stdio, tmpfs-backed regular files, and (in
//! later milestones) pipes and sockets.

use alloc::sync::Arc;

pub trait File: Send + Sync {
    fn readable(&self) -> bool {
        false
    }
    fn writable(&self) -> bool {
        false
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        let _ = buf;
        0
    }
    fn write(&self, buf: &[u8]) -> usize {
        let _ = buf;
        0
    }
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _ = (offset, buf);
        0
    }
    fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let _ = (offset, buf);
        0
    }
    fn size(&self) -> usize {
        0
    }
    fn is_dir(&self) -> bool {
        false
    }
    fn seek_to(&self, pos: usize) {
        let _ = pos;
    }
    fn tell(&self) -> usize {
        0
    }
    fn truncate(&self, len: usize) {
        let _ = len;
    }
    /// Unique per-inode identifier. musl's dynamic linker uses
    /// `(st_dev, st_ino)` to detect when two paths (e.g. reached via a
    /// symlink) name the same underlying file, so this must actually be
    /// distinct per file -- a constant here would make it think every
    /// shared object is a duplicate of the first one it loaded.
    fn ino(&self) -> u64 {
        0
    }
    /// Ready for a `read`/`recv`-style call to return data (or EOF)
    /// without blocking. Used by `epoll`/`select`-style polling; regular
    /// files and stdio are always considered ready.
    fn poll_readable(&self) -> bool {
        true
    }
    fn poll_writable(&self) -> bool {
        true
    }
    fn is_nonblocking(&self) -> bool {
        false
    }
    fn set_nonblocking(&self, v: bool) {
        let _ = v;
    }
    fn as_any(&self) -> &dyn core::any::Any;
}

mod regular;
mod stdio;
mod tar;
pub mod tmpfs;

pub use regular::{mkdir, open_file, stat_size_and_kind, unlink, O_APPEND, O_CREAT, O_DIRECTORY, O_EXCL, O_TRUNC};
pub use stdio::{Stdin, Stdout};

pub fn init() {
    tmpfs::init();
}

pub fn stdio_fd_table() -> alloc::vec::Vec<Option<Arc<dyn File>>> {
    alloc::vec![
        Some(Arc::new(Stdin) as Arc<dyn File>),
        Some(Arc::new(Stdout) as Arc<dyn File>),
        Some(Arc::new(Stdout) as Arc<dyn File>),
    ]
}

/// EAGAIN.
pub const EAGAIN: isize = -11;

/// Wait until `file` is ready to be read (or return `EAGAIN` immediately
/// for a non-blocking fd). Yields to the scheduler between checks, so
/// other tasks (importantly, whatever might make the fd ready) keep
/// running.
pub fn wait_readable(file: &Arc<dyn File>) -> Result<(), isize> {
    loop {
        if file.poll_readable() {
            return Ok(());
        }
        if file.is_nonblocking() {
            return Err(EAGAIN);
        }
        crate::task::suspend_current_and_run_next();
    }
}

pub fn wait_writable(file: &Arc<dyn File>) -> Result<(), isize> {
    loop {
        if file.poll_writable() {
            return Ok(());
        }
        if file.is_nonblocking() {
            return Err(EAGAIN);
        }
        crate::task::suspend_current_and_run_next();
    }
}
