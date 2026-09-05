use crate::fs::Inode;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub const O_CREAT: i32 = 0o100;
pub const O_TRUNC: i32 = 0o1000;
pub const O_APPEND: i32 = 0o2000;
pub const O_NONBLOCK: i32 = 0o4000;
pub const O_DIRECTORY: i32 = 0o200000;
pub const O_CLOEXEC: i32 = 0o2000000;
pub const O_RDONLY: i32 = 0;
pub const O_WRONLY: i32 = 1;
pub const O_RDWR: i32 = 2;

pub enum FileKind {
    Console,
    Null,
    Zero,
    Random,
    File(Arc<Mutex<Inode>>),
    Dir(Arc<Mutex<Inode>>),
    Socket(usize),
    Epoll(usize),
    Eventfd(u64),
}

pub struct FileDesc {
    pub kind: FileKind,
    pub offset: usize,
    pub flags: i32,
    pub readable: bool,
    pub writable: bool,
}

impl FileDesc {
    pub fn nonblocking(&self) -> bool {
        self.flags & O_NONBLOCK != 0
    }
}

pub type File = Arc<Mutex<FileDesc>>;

pub struct FdTable {
    pub files: Vec<Option<File>>,
    pub cloexec: Vec<bool>,
}

impl FdTable {
    pub fn new() -> Self {
        FdTable {
            files: Vec::new(),
            cloexec: Vec::new(),
        }
    }

    pub fn get(&self, fd: usize) -> Option<File> {
        self.files.get(fd).and_then(|f| f.clone())
    }

    fn ensure(&mut self, fd: usize) {
        while self.files.len() <= fd {
            self.files.push(None);
            self.cloexec.push(false);
        }
    }

    /// Allocate the lowest free fd >= min.
    pub fn alloc_from(&mut self, min: usize, file: File, cloexec: bool) -> usize {
        let mut fd = min;
        loop {
            self.ensure(fd);
            if self.files[fd].is_none() {
                self.files[fd] = Some(file);
                self.cloexec[fd] = cloexec;
                return fd;
            }
            fd += 1;
        }
    }

    pub fn alloc(&mut self, file: File, cloexec: bool) -> usize {
        self.alloc_from(0, file, cloexec)
    }

    pub fn set(&mut self, fd: usize, file: File, cloexec: bool) {
        self.ensure(fd);
        self.files[fd] = Some(file);
        self.cloexec[fd] = cloexec;
    }

    pub fn close(&mut self, fd: usize) -> bool {
        if fd < self.files.len() && self.files[fd].is_some() {
            self.files[fd] = None;
            true
        } else {
            false
        }
    }
}
