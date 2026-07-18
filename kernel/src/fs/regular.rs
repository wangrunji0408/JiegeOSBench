use super::tmpfs::{self, Inode};
use super::File;
use alloc::sync::Arc;
use spin::Mutex;

pub struct RegularFile {
    inode: Arc<Inode>,
    offset: Mutex<usize>,
    readable: bool,
    writable: bool,
    append: bool,
    /// Debugging aid: also echo writes to the kernel console. Real Linux
    /// obviously doesn't do this, but nginx's log files are otherwise
    /// invisible from outside the emulated machine, which makes debugging
    /// much harder than it needs to be.
    debug_echo: bool,
}

impl RegularFile {
    pub fn new(inode: Arc<Inode>, readable: bool, writable: bool, append: bool, debug_echo: bool) -> Self {
        Self {
            inode,
            offset: Mutex::new(0),
            readable,
            writable,
            append,
            debug_echo,
        }
    }
}

impl File for RegularFile {
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        let mut off = self.offset.lock();
        let n = self.read_at(*off, buf);
        *off += n;
        n
    }
    fn write(&self, buf: &[u8]) -> usize {
        let mut off = self.offset.lock();
        let pos = if self.append { self.size() } else { *off };
        let n = self.write_at(pos, buf);
        *off = pos + n;
        n
    }
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        match &*self.inode {
            Inode::File(data) => {
                let data = data.lock();
                if offset >= data.len() {
                    return 0;
                }
                let n = buf.len().min(data.len() - offset);
                buf[..n].copy_from_slice(&data[offset..offset + n]);
                n
            }
            _ => 0,
        }
    }
    fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        if self.debug_echo {
            if let Ok(s) = core::str::from_utf8(buf) {
                crate::print!("{}", s);
            }
        }
        match &*self.inode {
            Inode::File(data) => {
                let mut data = data.lock();
                let end = offset + buf.len();
                if data.len() < end {
                    data.resize(end, 0);
                }
                data[offset..end].copy_from_slice(buf);
                buf.len()
            }
            _ => 0,
        }
    }
    fn size(&self) -> usize {
        self.inode.size()
    }
    fn is_dir(&self) -> bool {
        self.inode.is_dir()
    }
    fn seek_to(&self, pos: usize) {
        *self.offset.lock() = pos;
    }
    fn tell(&self) -> usize {
        *self.offset.lock()
    }
    fn truncate(&self, len: usize) {
        if let Inode::File(data) = &*self.inode {
            data.lock().resize(len, 0);
        }
    }
    fn ino(&self) -> u64 {
        Arc::as_ptr(&self.inode) as *const () as u64
    }
}

pub const O_WRONLY: u32 = 0o1;
pub const O_RDWR: u32 = 0o2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_DIRECTORY: u32 = 0o200000;

fn is_log_path(path: &str) -> bool {
    path.ends_with(".log")
}

/// Open (optionally creating) a tmpfs path, returning a ready-to-use `File`.
pub fn open_file(path: &str, flags: u32) -> Option<Arc<dyn File>> {
    let writable = flags & (O_WRONLY | O_RDWR) != 0;
    let readable = flags & O_WRONLY == 0;
    let append = flags & O_APPEND != 0;

    let inode = match tmpfs::resolve(path) {
        Some(inode) => {
            if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
                return None; // EEXIST
            }
            if flags & O_TRUNC != 0 {
                if let Inode::File(data) = &*inode {
                    data.lock().clear();
                }
            }
            inode
        }
        None => {
            if flags & O_CREAT == 0 {
                return None;
            }
            let new_inode = Inode::new_file(alloc::vec::Vec::new());
            if !tmpfs::insert_absolute(path, new_inode.clone()) {
                return None;
            }
            new_inode
        }
    };
    Some(Arc::new(RegularFile::new(
        inode,
        readable,
        writable,
        append,
        is_log_path(path),
    )))
}

pub fn mkdir(path: &str) -> bool {
    tmpfs::make_dirs_absolute(path).is_some()
}

pub fn unlink(path: &str) -> bool {
    if let Some((parent, name)) = tmpfs::resolve_parent(path) {
        if let Inode::Dir(m) = &*parent {
            return m.lock().remove(&name).is_some();
        }
    }
    false
}

pub fn stat_size_and_kind(path: &str) -> Option<(usize, bool, u64)> {
    tmpfs::resolve(path).map(|i| (i.size(), i.is_dir(), Arc::as_ptr(&i) as *const () as u64))
}
