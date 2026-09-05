//! Open file descriptions and the FileOps trait.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU32, Ordering};

use super::vfs::{Dentry, NodeKind};
use crate::abi::*;
use crate::net::socket::SocketOps;
use crate::sync::SpinLock;
use crate::task::wait::WaitQueue;

pub struct DirEntryInfo {
    pub ino: u64,
    pub name: String,
    pub dtype: u8,
}

pub trait FileOps: Any + Send + Sync {
    /// Read at `off` (ignored for non-seekable objects).
    fn read_at(&self, _off: u64, _buf: &mut [u8], _file: &File) -> SysResult {
        Err(EINVAL)
    }
    fn write_at(&self, _off: u64, _buf: &[u8], _file: &File) -> SysResult {
        Err(EINVAL)
    }
    /// Current readiness (POLLIN/POLLOUT/POLLHUP/...).
    fn poll(&self) -> u32 {
        POLLIN | POLLOUT
    }
    fn wait_queue(&self) -> Option<&WaitQueue> {
        None
    }
    fn ioctl(&self, _cmd: u32, _arg: usize) -> SysResult {
        Err(ENOTTY)
    }
    fn stat(&self) -> Result<Stat, i32>;
    fn dentry(&self) -> Option<Arc<Dentry>> {
        None
    }
    fn seekable(&self) -> bool {
        false
    }
    fn size(&self) -> u64 {
        0
    }
    fn truncate(&self, _len: u64) -> Result<(), i32> {
        Err(EINVAL)
    }
    fn readdir(&self) -> Result<Vec<DirEntryInfo>, i32> {
        Err(ENOTDIR)
    }
    fn as_socket(&self) -> Option<&dyn SocketOps> {
        None
    }
    fn as_any(&self) -> &dyn Any;
    /// Called when the last reference to the open file description goes away.
    fn release(&self) {}
    fn is_tty(&self) -> bool {
        false
    }
}

pub struct File {
    pub ops: Arc<dyn FileOps>,
    pub flags: AtomicU32,
    pub pos: SpinLock<u64>,
    pub path: String,
}

impl File {
    pub fn new(ops: Arc<dyn FileOps>, flags: u32, path: String) -> Arc<File> {
        Arc::new(File { ops, flags: AtomicU32::new(flags), pos: SpinLock::new(0), path })
    }

    pub fn flags(&self) -> u32 {
        self.flags.load(Ordering::Relaxed)
    }

    pub fn set_flags(&self, f: u32) {
        self.flags.store(f, Ordering::Relaxed);
    }

    pub fn nonblock(&self) -> bool {
        self.flags() & O_NONBLOCK != 0
    }

    pub fn readable(&self) -> bool {
        let m = self.flags() & O_ACCMODE;
        m == O_RDONLY || m == O_RDWR
    }

    pub fn writable(&self) -> bool {
        let m = self.flags() & O_ACCMODE;
        m == O_WRONLY || m == O_RDWR
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn dentry(&self) -> Option<Arc<Dentry>> {
        self.ops.dentry()
    }

    pub fn read(&self, buf: &mut [u8]) -> SysResult {
        if !self.readable() {
            return Err(EBADF);
        }
        if self.ops.seekable() {
            let pos = *self.pos.lock();
            let n = self.ops.read_at(pos, buf, self)?;
            *self.pos.lock() = pos + n as u64;
            Ok(n)
        } else {
            self.ops.read_at(0, buf, self)
        }
    }

    pub fn write(&self, buf: &[u8]) -> SysResult {
        if !self.writable() {
            return Err(EBADF);
        }
        if self.ops.seekable() {
            let pos = if self.flags() & O_APPEND != 0 { self.ops.size() } else { *self.pos.lock() };
            let n = self.ops.write_at(pos, buf, self)?;
            *self.pos.lock() = pos + n as u64;
            Ok(n)
        } else {
            self.ops.write_at(0, buf, self)
        }
    }

    pub fn pread(&self, buf: &mut [u8], off: u64) -> SysResult {
        if !self.ops.seekable() {
            return Err(ESPIPE);
        }
        self.ops.read_at(off, buf, self)
    }

    pub fn pwrite(&self, buf: &[u8], off: u64) -> SysResult {
        if !self.ops.seekable() {
            return Err(ESPIPE);
        }
        self.ops.write_at(off, buf, self)
    }

    pub fn lseek(&self, off: i64, whence: i32) -> Result<u64, i32> {
        if !self.ops.seekable() {
            return Err(ESPIPE);
        }
        let mut pos = self.pos.lock();
        let base = match whence {
            SEEK_SET => 0i64,
            SEEK_CUR => *pos as i64,
            SEEK_END => self.ops.size() as i64,
            _ => return Err(EINVAL),
        };
        let np = base.checked_add(off).ok_or(EINVAL)?;
        if np < 0 {
            return Err(EINVAL);
        }
        *pos = np as u64;
        Ok(np as u64)
    }

    pub fn stat(&self) -> Result<Stat, i32> {
        self.ops.stat()
    }

    pub fn poll(&self) -> u32 {
        self.ops.poll()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        self.ops.release();
    }
}

// ---------------------------------------------------------------------------
// Regular files and directories backed by the ramfs tree.

pub struct RegFile {
    pub dentry: Arc<Dentry>,
}

impl FileOps for RegFile {
    fn read_at(&self, off: u64, buf: &mut [u8], _file: &File) -> SysResult {
        let NodeKind::File(data) = &self.dentry.kind else { return Err(EISDIR) };
        let data = data.lock();
        let off = off as usize;
        if off >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - off);
        buf[..n].copy_from_slice(&data[off..off + n]);
        Ok(n)
    }

    fn write_at(&self, off: u64, buf: &[u8], _file: &File) -> SysResult {
        let NodeKind::File(data) = &self.dentry.kind else { return Err(EISDIR) };
        let mut data = data.lock();
        let off = off as usize;
        let end = off + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[off..end].copy_from_slice(buf);
        drop(data);
        self.dentry.touch();
        Ok(buf.len())
    }

    fn stat(&self) -> Result<Stat, i32> {
        Ok(self.dentry.stat())
    }

    fn dentry(&self) -> Option<Arc<Dentry>> {
        Some(self.dentry.clone())
    }

    fn seekable(&self) -> bool {
        true
    }

    fn size(&self) -> u64 {
        self.dentry.size()
    }

    fn truncate(&self, len: u64) -> Result<(), i32> {
        let NodeKind::File(data) = &self.dentry.kind else { return Err(EISDIR) };
        data.lock().resize(len as usize, 0);
        self.dentry.touch();
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct DirFile {
    pub dentry: Arc<Dentry>,
}

impl FileOps for DirFile {
    fn read_at(&self, _off: u64, _buf: &mut [u8], _file: &File) -> SysResult {
        Err(EISDIR)
    }
    fn stat(&self) -> Result<Stat, i32> {
        Ok(self.dentry.stat())
    }
    fn dentry(&self) -> Option<Arc<Dentry>> {
        Some(self.dentry.clone())
    }
    fn seekable(&self) -> bool {
        true
    }
    fn readdir(&self) -> Result<Vec<DirEntryInfo>, i32> {
        let mut out = Vec::new();
        out.push(DirEntryInfo { ino: self.dentry.ino, name: String::from("."), dtype: DT_DIR });
        let pino = self.dentry.parent().map(|p| p.ino).unwrap_or(self.dentry.ino);
        out.push(DirEntryInfo { ino: pino, name: String::from(".."), dtype: DT_DIR });
        for (name, d) in self.dentry.children() {
            let dtype = match d.file_type() {
                S_IFDIR => DT_DIR,
                S_IFREG => DT_REG,
                S_IFLNK => DT_LNK,
                S_IFCHR => DT_CHR,
                S_IFBLK => DT_BLK,
                S_IFIFO => DT_FIFO,
                S_IFSOCK => DT_SOCK,
                _ => DT_UNKNOWN,
            };
            out.push(DirEntryInfo { ino: d.ino, name, dtype });
        }
        Ok(out)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A file opened with O_PATH or a symlink node: only stat/readlink work.
pub struct NodeFile {
    pub dentry: Arc<Dentry>,
}

impl FileOps for NodeFile {
    fn stat(&self) -> Result<Stat, i32> {
        Ok(self.dentry.stat())
    }
    fn dentry(&self) -> Option<Arc<Dentry>> {
        Some(self.dentry.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
