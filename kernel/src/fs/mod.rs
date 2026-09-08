//! VFS: an in-memory inode tree populated from the initramfs, plus tmpfs-style
//! writable directories for /tmp, /run and /var/log.

pub mod chardev;
pub mod cpio;
pub mod file;
pub mod pipe;

use crate::errno::*;
use crate::mm;
use crate::sync::SpinLock;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    CharDev,
    BlockDev,
    Fifo,
    Socket,
    /// /proc/self/fd/N - resolved at open() time
    FdLink(u32),
}

pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

pub enum Data {
    Static(&'static [u8]),
    Owned(Vec<u8>),
    None,
}

impl Data {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Data::Static(s) => s,
            Data::Owned(v) => v,
            Data::None => &[],
        }
    }
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
}

pub struct InodeInner {
    pub kind: Kind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub ino: u64,
    pub rdev: u32,
    pub data: Data,
    pub children: BTreeMap<String, Arc<Inode>>,
    pub link: String,
    pub mtime: i64,
    pub atime: i64,
    pub ctime: i64,
}

pub struct Inode {
    pub inner: SpinLock<InodeInner>,
}

unsafe impl Send for Inode {}
unsafe impl Sync for Inode {}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub size: i64,
    pub blksize: i64,
    pub blocks: i64,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
}

static NEXT_INO: SpinLock<u64> = SpinLock::new(1);
static ROOT: SpinLock<Option<Arc<Inode>>> = SpinLock::new(None);

fn alloc_ino() -> u64 {
    let mut n = NEXT_INO.lock();
    *n += 1;
    *n
}

impl Inode {
    pub fn new(kind: Kind, mode: u32) -> Arc<Self> {
        Arc::new(Inode {
            inner: SpinLock::new(InodeInner {
                kind,
                mode,
                uid: 0,
                gid: 0,
                nlink: 1,
                ino: alloc_ino(),
                rdev: 0,
                data: Data::None,
                children: BTreeMap::new(),
                link: String::new(),
                mtime: 0,
                atime: 0,
                ctime: 0,
            }),
        })
    }

    pub fn new_dir(mode: u32) -> Arc<Self> {
        Self::new(Kind::Dir, mode | S_IFDIR)
    }

    pub fn new_file(mode: u32, data: Data) -> Arc<Self> {
        let n = Self::new(Kind::File, mode | S_IFREG);
        n.inner.lock().data = data;
        n
    }

    pub fn new_chardev(rdev: u32) -> Arc<Self> {
        let n = Self::new(Kind::CharDev, 0o666 | S_IFCHR);
        n.inner.lock().rdev = rdev;
        n
    }

    pub fn new_symlink(target: &str) -> Arc<Self> {
        let n = Self::new(Kind::Symlink, 0o777 | S_IFLNK);
        n.inner.lock().link = target.to_string();
        n
    }

    pub fn kind(&self) -> Kind {
        self.inner.lock().kind
    }

    pub fn is_dir(&self) -> bool {
        self.inner.lock().kind == Kind::Dir
    }

    pub fn stat(&self) -> Stat {
        let i = self.inner.lock();
        let size = match i.kind {
            Kind::Dir => 4096,
            Kind::Symlink => i.link.len() as i64,
            _ => i.data.len() as i64,
        };
        Stat {
            dev: 0x1000,
            ino: i.ino,
            mode: i.mode,
            nlink: i.nlink,
            uid: i.uid,
            gid: i.gid,
            rdev: i.rdev as u64,
            size,
            blksize: 4096,
            blocks: (size + 511) / 512,
            atime: i.atime,
            mtime: i.mtime,
            ctime: i.ctime,
        }
    }

    pub fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let i = self.inner.lock();
        let d = i.data.as_slice();
        if off as usize >= d.len() {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len(), d.len() - off as usize);
        buf[..n].copy_from_slice(&d[off as usize..off as usize + n]);
        Ok(n)
    }

    pub fn write_at(&self, off: u64, buf: &[u8]) -> Result<usize, Errno> {
        let mut i = self.inner.lock();
        if i.kind != Kind::File {
            return Err(EINVAL);
        }
        let off = off as usize;
        let need = off + buf.len();
        match &mut i.data {
            Data::Owned(v) => {
                if v.len() < need {
                    v.resize(need, 0);
                }
                v[off..need].copy_from_slice(buf);
            }
            Data::None => {
                let mut v = alloc::vec![0u8; need];
                v[off..need].copy_from_slice(buf);
                i.data = Data::Owned(v);
            }
            Data::Static(_) => {
                let mut v = i.data.as_slice().to_vec();
                if v.len() < need {
                    v.resize(need, 0);
                }
                v[off..need].copy_from_slice(buf);
                i.data = Data::Owned(v);
            }
        }
        i.mtime = crate::time::realtime_ns() as i64 / 1_000_000_000;
        Ok(buf.len())
    }

    pub fn truncate(&self, size: u64) -> Result<(), Errno> {
        let mut i = self.inner.lock();
        let mut v = i.data.as_slice().to_vec();
        v.resize(size as usize, 0);
        i.data = Data::Owned(v);
        Ok(())
    }

    pub fn size(&self) -> u64 {
        self.inner.lock().data.len() as u64
    }

    pub fn readdir(&self, offset: u64) -> Result<Option<(String, u64, u8)>, Errno> {
        let i = self.inner.lock();
        if i.kind != Kind::Dir {
            return Err(ENOTDIR);
        }
        let mut idx = 0u64;
        for (name, child) in i.children.iter() {
            if idx >= offset {
                let c = child.inner.lock();
                let dtype = match c.kind {
                    Kind::Dir => 4u8,
                    Kind::File => 8u8,
                    Kind::Symlink => 10u8,
                    Kind::CharDev => 2u8,
                    Kind::BlockDev => 6u8,
                    Kind::Fifo => 1u8,
                    Kind::Socket => 12u8,
                    Kind::FdLink(_) => 10u8,
                };
                return Ok(Some((name.clone(), c.ino, dtype)));
            }
            idx += 1;
        }
        Ok(None)
    }

    pub fn child(&self, name: &str) -> Option<Arc<Inode>> {
        self.inner.lock().children.get(name).cloned()
    }

    pub fn add_child(&self, name: &str, node: Arc<Inode>) {
        self.inner.lock().children.insert(name.to_string(), node);
    }
}

pub fn root() -> Arc<Inode> {
    ROOT.lock().clone().expect("no root filesystem")
}

pub fn mkdir_path(root: &Arc<Inode>, path: &str) {
    let mut cur = root.clone();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        let next = match cur.child(comp) {
            Some(n) => n,
            None => {
                let n = Inode::new_dir(0o755);
                cur.add_child(comp, n.clone());
                n
            }
        };
        cur = next;
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("/", path),
    }
}

fn lookup_abs(root: &Arc<Inode>, path: &str) -> Option<Arc<Inode>> {
    let mut cur = root.clone();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        cur = cur.child(comp)?;
    }
    Some(cur)
}

/// Resolve `path` relative to `cwd`.
pub fn lookup(cwd: &Arc<Inode>, path: &str, follow: bool) -> Result<Arc<Inode>, Errno> {
    let root = root();
    let mut cur = if path.starts_with('/') {
        root.clone()
    } else {
        cwd.clone()
    };
    let mut links = 0;
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let n = comps.len();
    for (i, comp) in comps.iter().enumerate() {
        let node = match *comp {
            "." => continue,
            ".." => continue,
            name => cur.child(name).ok_or(ENOENT)?,
        };
        let kind = node.kind();
        if kind == Kind::Symlink && (follow || i + 1 < n) {
            links += 1;
            if links > 40 {
                return Err(ELOOP);
            }
            let target = node.inner.lock().link.clone();
            let mut newpath = target;
            for r in &comps[i + 1..] {
                newpath.push('/');
                newpath.push_str(r);
            }
            let base = if newpath.starts_with('/') {
                root.clone()
            } else {
                cur.clone()
            };
            return lookup(&base, &newpath, follow);
        }
        if i + 1 < n && kind != Kind::Dir {
            return Err(ENOTDIR);
        }
        cur = node;
    }
    Ok(cur)
}

/// Resolve the parent directory of `path` and the final component.
pub fn lookup_parent(cwd: &Arc<Inode>, path: &str) -> Result<(Arc<Inode>, String), Errno> {
    let (dir, name) = split_parent(path);
    let parent = if dir == "/" {
        root()
    } else {
        lookup(cwd, dir, true)?
    };
    Ok((parent, name.to_string()))
}

pub fn create_at(cwd: &Arc<Inode>, path: &str, mode: u32, kind: Kind) -> Result<Arc<Inode>, Errno> {
    let (parent, name) = lookup_parent(cwd, path)?;
    if name.is_empty() {
        return Err(EISDIR);
    }
    if !parent.is_dir() {
        return Err(ENOTDIR);
    }
    if let Some(existing) = parent.child(&name) {
        return Ok(existing);
    }
    let node = match kind {
        Kind::Dir => Inode::new_dir(mode & 0o7777),
        _ => Inode::new_file(mode & 0o7777, Data::Owned(Vec::new())),
    };
    parent.add_child(&name, node.clone());
    Ok(node)
}

pub fn unlink_at(cwd: &Arc<Inode>, path: &str) -> Result<(), Errno> {
    let (parent, name) = lookup_parent(cwd, path)?;
    let mut p = parent.inner.lock();
    match p.children.get(&name) {
        Some(n) => {
            if n.is_dir() && !n.inner.lock().children.is_empty() {
                return Err(ENOTEMPTY);
            }
            p.children.remove(&name);
            Ok(())
        }
        None => Err(ENOENT),
    }
}

pub fn rename_at(cwd: &Arc<Inode>, old: &str, new: &str) -> Result<(), Errno> {
    let (oldp, oldn) = lookup_parent(cwd, old)?;
    let (newp, newn) = lookup_parent(cwd, new)?;
    let node = oldp.child(&oldn).ok_or(ENOENT)?;
    oldp.inner.lock().children.remove(&oldn);
    newp.add_child(&newn, node);
    Ok(())
}

pub fn symlink_at(cwd: &Arc<Inode>, target: &str, linkpath: &str) -> Result<(), Errno> {
    let (parent, name) = lookup_parent(cwd, linkpath)?;
    parent.add_child(&name, Inode::new_symlink(target));
    Ok(())
}

pub fn link_at(cwd: &Arc<Inode>, old: &str, new: &str) -> Result<(), Errno> {
    let node = lookup(cwd, old, true)?;
    let (parent, name) = lookup_parent(cwd, new)?;
    parent.add_child(&name, node);
    Ok(())
}

pub fn initramfs_ready() -> bool {
    mm::memory_map().initrd_start != 0
}

pub fn init() {
    let root = Inode::new_dir(0o755);
    let mmap = mm::memory_map();
    if mmap.initrd_start != 0 && mmap.initrd_end > mmap.initrd_start {
        let len = mmap.initrd_end - mmap.initrd_start;
        let buf: &'static [u8] =
            unsafe { core::slice::from_raw_parts(mmap.initrd_start as *const u8, len) };
        crate::println!("[fs] loading initramfs: {} KiB", len / 1024);
        load_cpio(&root, buf);
    }

    for d in [
        "/tmp",
        "/run",
        "/var",
        "/var/log",
        "/var/log/nginx",
        "/var/lib",
        "/var/lib/nginx",
        "/var/lib/nginx/tmp",
        "/var/lib/nginx/tmp/client_body",
        "/var/cache",
        "/dev",
        "/proc",
        "/proc/self",
        "/proc/self/fd",
        "/sys",
        "/root",
        "/home",
        "/srv",
    ] {
        mkdir_path(&root, d);
    }

    let dev = lookup_abs(&root, "/dev").unwrap();
    for (name, rdev) in [
        ("null", chardev::DEV_NULL),
        ("zero", chardev::DEV_ZERO),
        ("full", chardev::DEV_FULL),
        ("random", chardev::DEV_RANDOM),
        ("urandom", chardev::DEV_URANDOM),
        ("console", chardev::DEV_CONSOLE),
        ("tty", chardev::DEV_CONSOLE),
        ("stderr", chardev::DEV_CONSOLE),
        ("stdout", chardev::DEV_CONSOLE),
        ("stdin", chardev::DEV_CONSOLE),
    ] {
        dev.add_child(name, Inode::new_chardev(rdev));
    }

    let fddir = lookup_abs(&root, "/proc/self/fd").unwrap();
    for n in 0..64u32 {
        fddir.add_child(&alloc::format!("{}", n), Inode::new(Kind::FdLink(n), 0o777 | S_IFLNK));
    }

    let proc = lookup_abs(&root, "/proc").unwrap();
    proc.add_child(
        "meminfo",
        Inode::new_file(
            0o444,
            Data::Static(b"MemTotal:        2000000 kB\nMemFree:         1500000 kB\nMemAvailable:    1500000 kB\n"),
        ),
    );
    proc.add_child(
        "cpuinfo",
        Inode::new_file(
            0o444,
            Data::Static(b"processor\t: 0\nhart\t\t: 0\nisa\t\t: rv64imafdc\nmmu\t\t: sv39\n"),
        ),
    );
    proc.add_child(
        "stat",
        Inode::new_file(
            0o444,
            Data::Static(b"cpu  0 0 0 0 0 0 0 0 0 0\ncpu0 0 0 0 0 0 0 0 0 0 0\nctxt 0\nbtime 0\nprocesses 1\nprocs_running 1\nprocs_blocked 0\n"),
        ),
    );
    proc.add_child(
        "version",
        Inode::new_file(0o444, Data::Static(b"Linux version 6.8.0-riscv64 (ijiege) #1 SMP\n")),
    );
    proc.add_child("mounts", Inode::new_file(0o444, Data::Static(b"none / rootfs rw 0 0\n")));

    *ROOT.lock() = Some(root);
    crate::println!("[fs] root filesystem ready");
}

fn load_cpio(root: &Arc<Inode>, buf: &'static [u8]) {
    let mut hardlinks: BTreeMap<u32, Arc<Inode>> = BTreeMap::new();
    let mut count = 0;
    cpio::walk(buf, |e| {
        let mode = e.mode;
        let ftype = mode & S_IFMT;
        let name = e.name.trim_start_matches("./").trim_start_matches('/');
        if name.is_empty() || name == "." {
            return;
        }
        let (dir, base) = split_parent(name);
        let parent = if dir.is_empty() || dir == "." {
            root.clone()
        } else {
            match lookup_abs(root, dir) {
                Some(p) => p,
                None => {
                    mkdir_path(root, dir);
                    lookup_abs(root, dir).unwrap()
                }
            }
        };
        // An archive may repeat a directory entry (e.g. once as a parent and
        // once explicitly); never replace a populated directory with an empty one.
        if let Some(existing) = parent.child(base) {
            if ftype == S_IFDIR && existing.is_dir() {
                return;
            }
        }
        if e.nlink > 1 && ftype == S_IFREG {
            if let Some(existing) = hardlinks.get(&e.ino) {
                existing.inner.lock().nlink += 1;
                parent.add_child(base, existing.clone());
                return;
            }
        }
        let node = match ftype {
            S_IFDIR => Inode::new_dir(mode & 0o7777),
            S_IFLNK => Inode::new_symlink(core::str::from_utf8(e.data).unwrap_or("")),
            S_IFCHR => Inode::new_chardev(e.rdev),
            S_IFBLK => Inode::new(Kind::BlockDev, (mode & 0o7777) | S_IFBLK),
            S_IFIFO => Inode::new(Kind::Fifo, (mode & 0o7777) | S_IFIFO),
            _ => Inode::new_file(mode & 0o7777, Data::Static(e.data)),
        };
        {
            let mut i = node.inner.lock();
            i.uid = e.uid;
            i.gid = e.gid;
            i.nlink = e.nlink.max(1);
            i.ino = e.ino as u64;
        }
        if e.nlink > 1 && ftype == S_IFREG {
            hardlinks.insert(e.ino, node.clone());
        }
        parent.add_child(base, node);
        count += 1;
    });
    crate::println!("[fs] initramfs: {} entries", count);
}
