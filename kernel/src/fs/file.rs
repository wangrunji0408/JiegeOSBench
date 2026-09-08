//! Open file descriptions and the per-process file descriptor table.

use super::{chardev, pipe, Inode, Stat};
use crate::errno::*;
use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

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
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_NOFOLLOW: u32 = 0o400000;
pub const O_CLOEXEC: u32 = 0o2000000;
pub const O_PATH: u32 = 0o10000000;
pub const O_TMPFILE: u32 = 0o20000000 | O_DIRECTORY;

pub const SEEK_SET: u32 = 0;
pub const SEEK_CUR: u32 = 1;
pub const SEEK_END: u32 = 2;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;

pub struct EventFd {
    pub count: SpinLock<u64>,
    pub semaphore: bool,
}

impl EventFd {
    pub fn new(semaphore: bool) -> Arc<Self> {
        Arc::new(Self {
            count: SpinLock::new(0),
            semaphore,
        })
    }
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        if buf.len() < 8 {
            return Err(EINVAL);
        }
        let mut c = self.count.lock();
        if *c == 0 {
            return Err(EAGAIN);
        }
        let v = if self.semaphore {
            *c -= 1;
            1u64
        } else {
            let v = *c;
            *c = 0;
            v
        };
        buf[..8].copy_from_slice(&v.to_le_bytes());
        Ok(8)
    }
    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        if buf.len() < 8 {
            return Err(EINVAL);
        }
        let v = u64::from_le_bytes(buf[..8].try_into().unwrap());
        let mut c = self.count.lock();
        if self.semaphore {
            if *c == u64::MAX {
                return Err(EAGAIN);
            }
            *c += 1;
        } else {
            if v == u64::MAX || *c > u64::MAX - v {
                return Err(EAGAIN);
            }
            *c += v;
        }
        Ok(8)
    }
    pub fn readable(&self) -> bool {
        *self.count.lock() > 0
    }
}

pub struct EpollItem {
    pub fd: i32,
    pub file: Arc<File>,
    pub events: u32,
    pub data: u64,
}

pub struct Epoll {
    pub items: SpinLock<Vec<EpollItem>>,
}

impl Epoll {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            items: SpinLock::new(Vec::new()),
        })
    }
    pub fn ctl(&self, op: i32, fd: i32, file: Arc<File>, events: u32, data: u64) -> Result<(), Errno> {
        let mut items = self.items.lock();
        match op {
            1 => {
                // EPOLL_CTL_ADD.  A stale entry can survive a close() when the
                // descriptor number is reused; replace it unless it is the very
                // same open file description.
                if let Some(pos) = items.iter().position(|i| i.fd == fd) {
                    if Arc::ptr_eq(&items[pos].file, &file) {
                        return Err(EEXIST);
                    }
                    items[pos] = EpollItem {
                        fd,
                        file,
                        events,
                        data,
                    };
                    return Ok(());
                }
                items.push(EpollItem {
                    fd,
                    file,
                    events,
                    data,
                });
                Ok(())
            }
            2 => {
                // EPOLL_CTL_DEL
                let n = items.len();
                items.retain(|i| i.fd != fd);
                if items.len() == n {
                    return Err(ENOENT);
                }
                Ok(())
            }
            3 => {
                // EPOLL_CTL_MOD
                for i in items.iter_mut() {
                    if i.fd == fd {
                        i.file = file;
                        i.events = events;
                        i.data = data;
                        return Ok(());
                    }
                }
                Err(ENOENT)
            }
            _ => Err(EINVAL),
        }
    }
}

pub enum FileObj {
    Regular {
        inode: Arc<Inode>,
        offset: u64,
    },
    Dir {
        inode: Arc<Inode>,
        offset: u64,
    },
    CharDev {
        rdev: u32,
    },
    Pipe {
        pipe: Arc<pipe::Pipe>,
        write_end: bool,
    },
    Socket(Arc<crate::socket::Socket>),
    Epoll(Arc<Epoll>),
    EventFd(Arc<EventFd>),
}

pub struct File {
    pub obj: SpinLock<FileObj>,
    pub status: SpinLock<u32>,
    pub cloexec: AtomicBool,
}

impl File {
    pub fn new(obj: FileObj, status: u32, cloexec: bool) -> Arc<Self> {
        Arc::new(Self {
            obj: SpinLock::new(obj),
            status: SpinLock::new(status),
            cloexec: AtomicBool::new(cloexec),
        })
    }

    pub fn inode_of(&self) -> Option<Arc<Inode>> {
        match &*self.obj.lock() {
            FileObj::Regular { inode, .. } | FileObj::Dir { inode, .. } => Some(inode.clone()),
            _ => None,
        }
    }

    pub fn nonblocking(&self) -> bool {
        *self.status.lock() & O_NONBLOCK != 0
    }

    pub fn is_dir(&self) -> bool {
        matches!(&*self.obj.lock(), FileObj::Dir { .. })
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        let mut o = self.obj.lock();
        match &mut *o {
            FileObj::Regular { inode, offset } => {
                let n = inode.read_at(*offset, buf)?;
                *offset += n as u64;
                Ok(n)
            }
            FileObj::Dir { .. } => Err(EISDIR),
            FileObj::CharDev { rdev } => chardev::read(*rdev, buf),
            FileObj::Pipe { pipe, write_end } => {
                if *write_end {
                    return Err(EBADF);
                }
                pipe.read(buf)
            }
            FileObj::Socket(s) => s.recv(buf),
            FileObj::Epoll(_) => Err(EINVAL),
            FileObj::EventFd(e) => e.read(buf),
        }
    }

    pub fn write(&self, buf: &[u8]) -> Result<usize, Errno> {
        let append = *self.status.lock() & O_APPEND != 0;
        let mut o = self.obj.lock();
        match &mut *o {
            FileObj::Regular { inode, offset } => {
                if append {
                    *offset = inode.size();
                }
                let n = inode.write_at(*offset, buf)?;
                *offset += n as u64;
                Ok(n)
            }
            FileObj::Dir { .. } => Err(EISDIR),
            FileObj::CharDev { rdev } => chardev::write(*rdev, buf),
            FileObj::Pipe { pipe, write_end } => {
                if !*write_end {
                    return Err(EBADF);
                }
                pipe.write(buf)
            }
            FileObj::Socket(s) => s.send(buf),
            FileObj::Epoll(_) => Err(EINVAL),
            FileObj::EventFd(e) => e.write(buf),
        }
    }

    pub fn lseek(&self, off: i64, whence: u32) -> Result<u64, Errno> {
        let mut o = self.obj.lock();
        match &mut *o {
            FileObj::Regular { inode, offset } => {
                let size = inode.size() as i64;
                let new = match whence {
                    SEEK_SET => off,
                    SEEK_CUR => *offset as i64 + off,
                    SEEK_END => size + off,
                    _ => return Err(EINVAL),
                };
                if new < 0 {
                    return Err(EINVAL);
                }
                *offset = new as u64;
                Ok(new as u64)
            }
            FileObj::Dir { offset, .. } => {
                let new = match whence {
                    SEEK_SET => off,
                    SEEK_CUR => *offset as i64 + off,
                    SEEK_END => off,
                    _ => return Err(EINVAL),
                };
                if new < 0 {
                    return Err(EINVAL);
                }
                *offset = new as u64;
                Ok(new as u64)
            }
            FileObj::CharDev { .. } | FileObj::Pipe { .. } => Err(ESPIPE),
            _ => Err(ESPIPE),
        }
    }

    pub fn stat(&self) -> Result<Stat, Errno> {
        let o = self.obj.lock();
        match &*o {
            FileObj::Regular { inode, .. } | FileObj::Dir { inode, .. } => Ok(inode.stat()),
            FileObj::CharDev { rdev } => {
                let mut s = Stat::default();
                s.mode = super::S_IFCHR | 0o666;
                s.rdev = *rdev as u64;
                s.nlink = 1;
                s.blksize = 4096;
                Ok(s)
            }
            FileObj::Pipe { .. } => {
                let mut s = Stat::default();
                s.mode = super::S_IFIFO | 0o600;
                s.nlink = 1;
                s.blksize = 4096;
                Ok(s)
            }
            FileObj::Socket(_) => {
                let mut s = Stat::default();
                s.mode = super::S_IFSOCK | 0o777;
                s.nlink = 1;
                s.blksize = 4096;
                Ok(s)
            }
            _ => {
                let mut s = Stat::default();
                s.mode = super::S_IFREG | 0o600;
                s.nlink = 1;
                s.blksize = 4096;
                Ok(s)
            }
        }
    }

    /// EPOLL* readiness bits.
    pub fn readiness(&self) -> u32 {
        let o = self.obj.lock();
        match &*o {
            FileObj::Regular { .. } | FileObj::Dir { .. } => EPOLLIN | EPOLLOUT,
            FileObj::CharDev { rdev } => {
                let mut r = 0;
                if chardev::readable(*rdev) {
                    r |= EPOLLIN;
                }
                if chardev::writable(*rdev) {
                    r |= EPOLLOUT;
                }
                r
            }
            FileObj::Pipe { pipe, write_end } => {
                let mut r = 0;
                if pipe.readable() {
                    r |= EPOLLIN;
                }
                if pipe.writable() {
                    r |= EPOLLOUT;
                }
                if *write_end && *pipe.readers.lock() == 0 {
                    r |= EPOLLERR;
                }
                if !*write_end && *pipe.writers.lock() == 0 {
                    r |= EPOLLHUP | EPOLLIN;
                }
                r
            }
            FileObj::Socket(s) => s.readiness(),
            FileObj::EventFd(e) => {
                if e.readable() {
                    EPOLLIN | EPOLLOUT
                } else {
                    EPOLLOUT
                }
            }
            FileObj::Epoll(_) => EPOLLIN | EPOLLOUT,
        }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        match &*self.obj.lock() {
            FileObj::Pipe { pipe, write_end } => {
                if *write_end {
                    pipe.close_writer();
                } else {
                    pipe.close_reader();
                }
            }
            FileObj::Socket(s) => s.on_close(),
            _ => {}
        }
    }
}

/// Per-process descriptor table.
pub struct FdTable {
    pub files: Vec<Option<Arc<File>>>,
}

impl FdTable {
    pub fn new() -> Self {
        let mut files = Vec::with_capacity(64);
        for _ in 0..64 {
            files.push(None);
        }
        Self { files }
    }

    pub fn get(&self, fd: i32) -> Result<Arc<File>, Errno> {
        if fd < 0 {
            return Err(EBADF);
        }
        self.files
            .get(fd as usize)
            .and_then(|f| f.clone())
            .ok_or(EBADF)
    }

    pub fn get_opt(&self, fd: i32) -> Option<Arc<File>> {
        if fd < 0 {
            return None;
        }
        self.files.get(fd as usize).and_then(|f| f.clone())
    }

    pub fn set(&mut self, fd: i32, f: Arc<File>) {
        if fd as usize >= self.files.len() {
            self.files.resize(fd as usize + 1, None);
        }
        self.files[fd as usize] = Some(f);
    }

    pub fn alloc(&mut self, f: Arc<File>, min: i32) -> Result<i32, Errno> {
        let mut i = min.max(0) as usize;
        while i < self.files.len() {
            if self.files[i].is_none() {
                self.files[i] = Some(f);
                return Ok(i as i32);
            }
            i += 1;
        }
        if self.files.len() >= 4096 {
            return Err(EMFILE);
        }
        let fd = self.files.len() as i32;
        self.files.push(Some(f));
        Ok(fd)
    }

    pub fn close(&mut self, fd: i32) -> Result<(), Errno> {
        if fd < 0 || fd as usize >= self.files.len() || self.files[fd as usize].is_none() {
            return Err(EBADF);
        }
        self.files[fd as usize] = None;
        Ok(())
    }

    pub fn clone_table(&self) -> Self {
        Self {
            files: self.files.clone(),
        }
    }

    pub fn close_cloexec(&mut self) {
        for f in self.files.iter_mut() {
            if let Some(file) = f {
                if file.cloexec.load(Ordering::Relaxed) {
                    *f = None;
                }
            }
        }
    }
}
