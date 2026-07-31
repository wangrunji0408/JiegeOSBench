//! In-memory filesystem (ramfs) built from an embedded cpio initramfs,
//! plus the per-process file descriptor table and the global file table.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::kprintln;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_ACCMODE: u32 = 3;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_CLOEXEC: u32 = 0o2000000;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;

pub const AT_FDCWD: isize = -100;

pub struct SharedFile {
    pub data: Vec<u8>,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub is_dir: bool,
    pub entries: Option<BTreeMap<String, usize>>,
}

impl SharedFile {
    pub fn new_dir(mode: u32) -> SharedFile {
        SharedFile {
            data: Vec::new(),
            mode: S_IFDIR | mode,
            nlink: 2,
            uid: 0,
            gid: 0,
            mtime: 0,
            is_dir: true,
            entries: Some(BTreeMap::new()),
        }
    }
    pub fn new_file(mode: u32, data: Vec<u8>) -> SharedFile {
        SharedFile {
            data,
            mode: S_IFREG | mode,
            nlink: 1,
            uid: 0,
            gid: 0,
            mtime: 0,
            is_dir: false,
            entries: None,
        }
    }
}

pub struct Fs {
    pub files: Vec<Option<Rc<RefCell<SharedFile>>>>,
}

impl Fs {
    fn add(&mut self, f: SharedFile) -> usize {
        self.files.push(Some(Rc::new(RefCell::new(f))));
        self.files.len() - 1
    }

    pub fn get(&self, id: usize) -> Option<Rc<RefCell<SharedFile>>> {
        self.files.get(id).and_then(|x| x.clone())
    }
}

pub static mut FS: Option<Fs> = None;
pub static mut NEXT_INO: u64 = 2;

pub fn fs() -> &'static mut Fs {
    unsafe { FS.as_mut().unwrap() }
}

pub fn new_ino() -> u64 {
    unsafe {
        NEXT_INO += 1;
        NEXT_INO
    }
}

// ---------- path resolution ----------

fn split_path(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_string())
        .collect()
}

/// Resolve an absolute path to a file id.
pub fn resolve_abs(path: &str) -> Option<usize> {
    let fs = fs();
    let mut cur = 0usize; // root
    for comp in split_path(path) {
        if comp == ".." {
            return None; // no parent tracking; not needed for our use
        }
        let file = fs.get(cur)?;
        let entries = file.borrow().entries.clone()?;
        match entries.get(&comp) {
            Some(&id) => cur = id,
            None => return None,
        }
    }
    Some(cur)
}

/// Resolve a path relative to cwd.
pub fn resolve(cwd: &str, path: &str) -> Option<usize> {
    if path.starts_with('/') {
        resolve_abs(path)
    } else {
        let joined = if cwd == "/" {
            format!("/{}", path)
        } else {
            format!("{}/{}", cwd, path)
        };
        resolve_abs(&joined)
    }
}

pub fn insert_file(parent_id: usize, name: &str, f: SharedFile) -> Option<usize> {
    let fs = fs();
    let id = fs.add(f);
    let parent = fs.get(parent_id)?;
    let mut p = parent.borrow_mut();
    if let Some(entries) = p.entries.as_mut() {
        entries.insert(name.to_string(), id);
        Some(id)
    } else {
        None
    }
}

pub fn mkdir_at(cwd: &str, path: &str, mode: u32) -> Result<usize, i32> {
    let parts = split_path(path);
    if parts.is_empty() {
        return Err(-17); // EEXIST
    }
    let (parent, name) = split_parent(cwd, path);
    let pid = resolve(cwd, &parent).ok_or(-2)?; // ENOENT
    let pfile = fs().get(pid).unwrap();
    let exists = pfile
        .borrow()
        .entries
        .as_ref()
        .unwrap()
        .contains_key(&name);
    if exists {
        return Err(-17);
    }
    insert_file(pid, &name, SharedFile::new_dir(mode & 0o7777))
        .ok_or(-13) // EACCES
}

pub fn split_parent(cwd: &str, path: &str) -> (String, String) {
    let parts = split_path(path);
    let name = parts.last().unwrap().clone();
    let parent = parts[..parts.len() - 1].join("/");
    let parent = if parent.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parent)
    };
    (parent, name)
}

pub fn create_at(cwd: &str, path: &str, mode: u32) -> Result<usize, i32> {
    let (parent, name) = split_parent(cwd, path);
    let pid = resolve(cwd, &parent).ok_or(-2)?;
    let f = SharedFile::new_file(mode & 0o7777, Vec::new());
    let id = insert_file(pid, &name, f);
    id.ok_or(-13) // EACCES
}

pub fn unlink_at(cwd: &str, path: &str) -> Result<(), i32> {
    let (parent, name) = split_parent(cwd, path);
    let pid = resolve(cwd, &parent).ok_or(-2)?;
    let pfile = fs().get(pid).unwrap();
    let mut p = pfile.borrow_mut();
    if let Some(entries) = p.entries.as_mut() {
        if entries.remove(&name).is_some() {
            Ok(())
        } else {
            Err(-2)
        }
    } else {
        Err(-20) // ENOTDIR
    }
}

// ---------- fd table ----------

#[derive(Clone)]
pub enum FdKind {
    File { file_id: usize },
    Console,
    Null,
    Socket { sock_id: usize },
    Epoll { ep_id: usize },
    Eventfd { counter: u64, flags: u32 },
    UnixPair { sock_id: usize },
}

#[derive(Clone)]
pub struct Fd {
    pub kind: FdKind,
    pub flags: u32,
    pub offset: u64,
    pub cloexec: bool,
    pub epoll: Option<(usize, u32, u64)>, // (epoll id, events, data)
}

#[derive(Clone)]
pub struct FdTable {
    pub fds: Vec<Option<Fd>>,
}

impl FdTable {
    pub fn new() -> FdTable {
        FdTable { fds: Vec::new() }
    }

    pub fn alloc(&mut self) -> Option<usize> {
        for (i, f) in self.fds.iter_mut().enumerate() {
            if f.is_none() {
                *f = Some(Fd {
                    kind: FdKind::Null,
                    flags: 0,
                    offset: 0,
                    cloexec: false,
                    epoll: None,
                });
                return Some(i);
            }
        }
        self.fds.push(Some(Fd {
            kind: FdKind::Null,
            flags: 0,
            offset: 0,
            cloexec: false,
            epoll: None,
        }));
        Some(self.fds.len() - 1)
    }

    pub fn install(&mut self, fd: usize, f: Fd) {
        while self.fds.len() <= fd {
            self.fds.push(None);
        }
        self.fds[fd] = Some(f);
    }

    pub fn get(&self, fd: usize) -> Option<&Fd> {
        self.fds.get(fd).and_then(|x| x.as_ref())
    }

    pub fn get_mut(&mut self, fd: usize) -> Option<&mut Fd> {
        self.fds.get_mut(fd).and_then(|x| x.as_mut())
    }

    pub fn close(&mut self, fd: usize) -> bool {
        if fd < self.fds.len() && self.fds[fd].is_some() {
            self.fds[fd] = None;
            true
        } else {
            false
        }
    }
}

// ---------- io helpers ----------

pub fn write_fd(fd: &mut Fd, buf: &[u8]) -> Result<usize, i32> {
    match &mut fd.kind {
        FdKind::File { file_id } => {
            let f = fs().get(*file_id).ok_or(-9)?; // EBADF
            let mut file = f.borrow_mut();
            if fd.flags & O_APPEND != 0 {
                fd.offset = file.data.len() as u64;
            }
            let off = fd.offset as usize;
            if off + buf.len() > file.data.len() {
                file.data.resize(off + buf.len(), 0);
            }
            file.data[off..off + buf.len()].copy_from_slice(buf);
            file.mtime = crate::timer::now_ms();
            fd.offset += buf.len() as u64;
            Ok(buf.len())
        }
        FdKind::Console => {
            crate::uart::puts(core::str::from_utf8(buf).unwrap_or("?"));
            Ok(buf.len())
        }
        FdKind::Null => Ok(buf.len()),
        FdKind::Eventfd { counter, .. } => {
            if buf.len() >= 8 {
                let v = u64::from_le_bytes(buf[..8].try_into().unwrap());
                *counter += v;
            }
            Ok(buf.len())
        }
        FdKind::Socket { sock_id } => crate::net::sock_write(*sock_id, buf),
        FdKind::UnixPair { sock_id } => crate::net::sock_write(*sock_id, buf),
        FdKind::Epoll { .. } => Err(-22), // EINVAL
    }
}

pub fn read_fd(fd: &mut Fd, buf: &mut [u8]) -> Result<usize, i32> {
    match &mut fd.kind {
        FdKind::File { file_id } => {
            let f = fs().get(*file_id).ok_or(-9)?;
            let file = f.borrow();
            let off = fd.offset as usize;
            if off >= file.data.len() {
                return Ok(0);
            }
            let n = core::cmp::min(buf.len(), file.data.len() - off);
            buf[..n].copy_from_slice(&file.data[off..off + n]);
            fd.offset += n as u64;
            Ok(n)
        }
        FdKind::Console => Ok(0), // EOF
        FdKind::Null => Ok(0),
        FdKind::Eventfd { counter, .. } => {
            if *counter == 0 && fd.flags & O_NONBLOCK != 0 {
                return Err(-11); // EAGAIN
            }
            let v = core::mem::replace(counter, 0);
            buf[..8].copy_from_slice(&v.to_le_bytes());
            Ok(8)
        }
        FdKind::Socket { sock_id } => crate::net::sock_read(*sock_id, buf),
        FdKind::UnixPair { sock_id } => crate::net::sock_read(*sock_id, buf),
        FdKind::Epoll { .. } => Err(-22),
    }
}

pub fn dup_fd(fd: &Fd) -> Fd {
    fd.clone()
}

/// Fill a Linux `struct stat` (riscv64 layout, exactly 128 bytes).
pub fn fill_stat(fd: &Fd, out: &mut [u8]) -> Result<(), i32> {
    if out.len() < 128 {
        return Err(-22);
    }
    let mut stat = [0u8; 128];
    let (dev, ino, mode, nlink, size, blksize, blocks) = match &fd.kind {
        FdKind::File { file_id } => {
            let f = fs().get(*file_id).ok_or(-9)?;
            let file = f.borrow();
            (
                0x8000u64,
                new_ino(),
                file.mode as u64,
                file.nlink as u64,
                file.data.len() as u64,
                4096u64,
                (file.data.len() as u64 + 511) / 512,
            )
        }
        FdKind::Console => (0x1000, 3, S_IFCHR as u64 | 0o600, 1, 0, 4096, 0),
        FdKind::Null => (0x1001, 4, S_IFCHR as u64 | 0o666, 1, 0, 4096, 0),
        FdKind::Socket { .. } | FdKind::UnixPair { .. } => {
            (0, 5, S_IFSOCK as u64 | 0o777, 1, 0, 4096, 0)
        }
        FdKind::Epoll { .. } => (0, 6, S_IFREG as u64 | 0o600, 1, 0, 4096, 0),
        FdKind::Eventfd { .. } => (0, 7, S_IFREG as u64 | 0o600, 1, 8, 4096, 0),
    };
    let mut put = |off: usize, v: u64| {
        stat[off..off + 8].copy_from_slice(&v.to_le_bytes());
    };
    put(0, dev);
    put(8, ino);
    put(16, mode); // st_mode (4 bytes used)
    put(20, nlink);
    put(24, 0); // uid
    put(28, 0); // gid
    put(32, 0); // rdev
    put(40, 0); // __pad1
    put(48, size);
    put(56, blksize);
    put(64, blocks);
    put(72, 0); // atime
    put(80, 0); // atime_nsec
    put(88, 0); // mtime
    put(96, 0); // mtime_nsec
    put(104, 0); // ctime
    put(112, 0); // ctime_nsec
    // 120..128: __unused4, __unused5 (zero)
    out[..128].copy_from_slice(&stat);
    Ok(())
}

/// Fill a `struct stat` from a path (for newfstatat). Exactly 128 bytes.
pub fn fill_stat_path(cwd: &str, path: &str, out: &mut [u8]) -> Result<(), i32> {
    let id = resolve(cwd, path).ok_or(-2)?;
    let f = fs().get(id).ok_or(-9)?;
    let file = f.borrow();
    if out.len() < 128 {
        return Err(-22);
    }
    let mut stat = [0u8; 128];
    let mode = file.mode as u64;
    let size = file.data.len() as u64;
    let blocks = (size + 511) / 512;
    let mut put = |off: usize, v: u64| {
        stat[off..off + 8].copy_from_slice(&v.to_le_bytes());
    };
    put(0, 0x8000);
    put(8, id as u64 + 1);
    put(16, mode);
    put(20, file.nlink as u64);
    put(48, size);
    put(56, 4096);
    put(64, blocks);
    out[..128].copy_from_slice(&stat);
    Ok(())
}

pub fn getdents(fd: &mut Fd, buf: &mut [u8]) -> Result<usize, i32> {
    let file_id = match &fd.kind {
        FdKind::File { file_id } => *file_id,
        _ => return Err(-20), // ENOTDIR
    };
    let f = fs().get(file_id).ok_or(-9)?;
    let file = f.borrow();
    let entries = match &file.entries {
        Some(e) => e,
        None => return Err(-20),
    };
    let mut off = fd.offset as usize;
    let mut written = 0usize;
    let mut skipped = 0usize;
    // iterate entries in order, skip already-consumed (offset counts entries)
    for (name, &id) in entries.iter() {
        if skipped < off {
            skipped += 1;
            continue;
        }
        let name_bytes = name.as_bytes();
        let reclen = (24 + name_bytes.len() + 1 + 7) & !7;
        if written + reclen > buf.len() {
            break;
        }
        let ent = &mut buf[written..written + reclen];
        ent[..8].copy_from_slice(&((id as u64 + 1).to_le_bytes())); // ino
        ent[8..16].copy_from_slice(&(((off + 1) as u64).to_le_bytes())); // off (dummy)
        ent[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        let subtype = if entries.get(name).map(|x| fs().get(*x).unwrap().borrow().is_dir).unwrap_or(false) {
            4u8
        } else {
            8u8
        };
        ent[18] = subtype;
        ent[19..19 + name_bytes.len()].copy_from_slice(name_bytes);
        ent[19 + name_bytes.len()] = 0;
        written += reclen;
        off += 1;
    }
    fd.offset = off as u64;
    Ok(written)
}

// ---------- initramfs (cpio newc) unpacking ----------

pub fn unpack_cpio(data: &[u8]) {
    let fsm = fs();
    // root
    let root_id = fsm.add(SharedFile::new_dir(0o755));
    debug_assert_eq!(root_id, 0);
    let mut p = 0usize;
    while p + 110 <= data.len() {
        let magic = &data[p..p + 6];
        if magic != b"070701" {
            kprintln!("[cpio] bad magic at {:#x}", p);
            break;
        }
        let rd = |off: usize| -> usize {
            usize::from_str_radix(core::str::from_utf8(&data[p + off..p + off + 8]).unwrap_or("0"), 16).unwrap_or(0)
        };
        let namesize = rd(94);
        let filesize = rd(54);
        let mode = rd(14) as u32;
        let name_start = p + 110;
        let name_end = name_start + namesize;
        let name_bytes = &data[name_start..name_end - 1]; // strip NUL
        let name = core::str::from_utf8(name_bytes).unwrap_or("");
        let data_start = (name_end + 3) & !3;
        let file_data = &data[data_start..data_start + filesize];
        if name.is_empty() || name == "." {
            p = (data_start + filesize + 3) & !3;
            continue;
        }
        if name == "TRAILER!!!" {
            break;
        }
        let path = name.trim_start_matches("./");
        let mut cur = 0usize; // root
        let comps: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for (i, comp) in comps.iter().enumerate() {
            let last = i == comps.len() - 1;
            let parent = fs().get(cur).unwrap();
            let exists = parent
                .borrow()
                .entries
                .as_ref()
                .unwrap()
                .contains_key(*comp);
            if exists {
                cur = parent
                    .borrow()
                    .entries
                    .as_ref()
                    .unwrap()
                    .get(*comp)
                    .unwrap()
                    .clone();
                continue;
            }
            if last {
                if mode & S_IFDIR != 0 {
                    cur = insert_file(cur, comp, SharedFile::new_dir(mode & 0o7777)).unwrap();
                } else {
                    cur = insert_file(cur, comp, SharedFile::new_file(mode & 0o7777, file_data.to_vec())).unwrap();
                }
            } else {
                cur = insert_file(cur, comp, SharedFile::new_dir(0o755)).unwrap();
            }
        }
        p = (data_start + filesize + 3) & !3;
    }
    kprintln!("[fs] initramfs unpacked");
}
