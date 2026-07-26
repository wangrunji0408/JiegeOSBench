//! A writable in-memory filesystem.
//!
//! File contents live in a `Vec<u8>` on the kernel heap; directories hold a map
//! from name to child inode. This backs `/` (holding nginx and its libraries),
//! `/tmp`, and `/run`.

use super::inode::{next_ino, DirEntry, Inode, InodeKind, InodeRef};
use super::{Error, Result};
use crate::{bail, impl_as_any};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::RwLock;

/// A regular file.
pub struct RamFile {
    ino: u64,
    mode: AtomicU32,
    uid: AtomicU32,
    gid: AtomicU32,
    data: RwLock<Vec<u8>>,
}

impl RamFile {
    pub fn new(mode: u32) -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            mode: AtomicU32::new(mode & 0o7777),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            data: RwLock::new(Vec::new()),
        })
    }

    /// Build a file whose contents are already known (used when unpacking the
    /// rootfs archive).
    pub fn with_data(mode: u32, data: Vec<u8>) -> Arc<Self> {
        let f = Self::new(mode);
        *f.data.write() = data;
        f
    }
}

impl Inode for RamFile {
    fn kind(&self) -> InodeKind {
        InodeKind::File
    }

    fn ino(&self) -> u64 {
        self.ino
    }

    fn size(&self) -> usize {
        self.data.read().len()
    }

    fn mode(&self) -> u32 {
        self.mode.load(Ordering::Relaxed)
    }

    fn set_mode(&self, mode: u32) {
        self.mode.store(mode & 0o7777, Ordering::Relaxed);
    }

    fn owner(&self) -> (u32, u32) {
        (self.uid.load(Ordering::Relaxed), self.gid.load(Ordering::Relaxed))
    }

    fn set_owner(&self, uid: u32, gid: u32) {
        if uid != u32::MAX {
            self.uid.store(uid, Ordering::Relaxed);
        }
        if gid != u32::MAX {
            self.gid.store(gid, Ordering::Relaxed);
        }
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let data = self.data.read();
        if offset >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - offset);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        Ok(n)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut data = self.data.write();
        let end = offset + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[offset..end].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn truncate(&self, len: usize) -> Result<()> {
        self.data.write().resize(len, 0);
        Ok(())
    }

    impl_as_any!();
}

/// A directory.
pub struct RamDir {
    ino: u64,
    mode: AtomicU32,
    uid: AtomicU32,
    gid: AtomicU32,
    children: RwLock<BTreeMap<String, InodeRef>>,
    /// `..` — weak so parent/child cycles don't leak.
    parent: RwLock<Weak<RamDir>>,
    this: RwLock<Weak<RamDir>>,
}

impl RamDir {
    pub fn new(mode: u32) -> Arc<Self> {
        let dir = Arc::new(Self {
            ino: next_ino(),
            mode: AtomicU32::new(mode & 0o7777),
            uid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(Weak::new()),
            this: RwLock::new(Weak::new()),
        });
        *dir.this.write() = Arc::downgrade(&dir);
        dir
    }

    pub fn new_root() -> InodeRef {
        let root = Self::new(0o755);
        // Root's parent is itself.
        *root.parent.write() = Arc::downgrade(&root);
        root
    }

    fn set_parent(&self, parent: &Arc<RamDir>) {
        *self.parent.write() = Arc::downgrade(parent);
    }

    /// Insert a child, replacing any existing entry of the same name.
    pub fn insert(&self, name: &str, inode: InodeRef) {
        if let Some(dir) = inode.as_any().downcast_ref::<RamDir>() {
            if let Some(me) = self.this.read().upgrade() {
                dir.set_parent(&me);
            }
        }
        self.children.write().insert(name.to_string(), inode);
    }
}

impl Inode for RamDir {
    fn kind(&self) -> InodeKind {
        InodeKind::Dir
    }

    fn ino(&self) -> u64 {
        self.ino
    }

    fn size(&self) -> usize {
        4096
    }

    fn mode(&self) -> u32 {
        self.mode.load(Ordering::Relaxed)
    }

    fn set_mode(&self, mode: u32) {
        self.mode.store(mode & 0o7777, Ordering::Relaxed);
    }

    fn owner(&self) -> (u32, u32) {
        (self.uid.load(Ordering::Relaxed), self.gid.load(Ordering::Relaxed))
    }

    fn set_owner(&self, uid: u32, gid: u32) {
        if uid != u32::MAX {
            self.uid.store(uid, Ordering::Relaxed);
        }
        if gid != u32::MAX {
            self.gid.store(gid, Ordering::Relaxed);
        }
    }

    fn lookup(&self, name: &str) -> Result<InodeRef> {
        match name {
            "" | "." => {
                return self
                    .this
                    .read()
                    .upgrade()
                    .map(|d| d as InodeRef)
                    .ok_or(Error::new(super::errno::ENOENT))
            }
            ".." => {
                return self
                    .parent
                    .read()
                    .upgrade()
                    .map(|d| d as InodeRef)
                    .ok_or(Error::new(super::errno::ENOENT))
            }
            _ => {}
        }
        self.children
            .read()
            .get(name)
            .cloned()
            .ok_or(Error::new(super::errno::ENOENT))
    }

    fn create(&self, name: &str, kind: InodeKind, mode: u32) -> Result<InodeRef> {
        let mut children = self.children.write();
        if children.contains_key(name) {
            bail!(EEXIST);
        }
        let inode: InodeRef = match kind {
            InodeKind::File => RamFile::new(mode),
            InodeKind::Dir => {
                let dir = RamDir::new(mode);
                if let Some(me) = self.this.read().upgrade() {
                    dir.set_parent(&me);
                }
                dir
            }
            InodeKind::Fifo => super::pipe::FifoInode::new(mode),
            // nginx's `unix:` listeners would need real socket inodes; we create
            // a placeholder so `bind` can name it, since the HTTP path never
            // uses them.
            InodeKind::Socket => RamFile::new(mode),
            _ => bail!(EPERM),
        };
        children.insert(name.to_string(), inode.clone());
        Ok(inode)
    }

    fn link(&self, name: &str, inode: &InodeRef) -> Result<()> {
        let mut children = self.children.write();
        if children.contains_key(name) {
            bail!(EEXIST);
        }
        children.insert(name.to_string(), inode.clone());
        Ok(())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let mut children = self.children.write();
        let Some(inode) = children.get(name) else {
            bail!(ENOENT);
        };
        if inode.kind() == InodeKind::Dir {
            // rmdir semantics: only empty directories.
            if let Some(dir) = inode.as_any().downcast_ref::<RamDir>() {
                if !dir.children.read().is_empty() {
                    bail!(ENOTEMPTY);
                }
            }
        }
        children.remove(name);
        Ok(())
    }

    fn rename(&self, old: &str, new_dir: &InodeRef, new: &str) -> Result<()> {
        let inode = self.lookup(old)?;
        // Take the source out first so a rename within one directory doesn't
        // deadlock on `children`.
        self.children.write().remove(old);
        if let Some(dst) = new_dir.as_any().downcast_ref::<RamDir>() {
            dst.insert(new, inode);
            Ok(())
        } else {
            // Put it back; the destination isn't a ramfs directory.
            self.children.write().insert(old.to_string(), inode);
            bail!(EXDEV)
        }
    }

    fn readdir(&self) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        out.push(DirEntry {
            name: ".".to_string(),
            kind: InodeKind::Dir,
            ino: self.ino,
        });
        out.push(DirEntry {
            name: "..".to_string(),
            kind: InodeKind::Dir,
            ino: self.parent.read().upgrade().map(|p| p.ino).unwrap_or(self.ino),
        });
        for (name, inode) in self.children.read().iter() {
            out.push(DirEntry {
                name: name.clone(),
                kind: inode.kind(),
                ino: inode.ino(),
            });
        }
        Ok(out)
    }

    fn symlink(&self, name: &str, target: &str) -> Result<InodeRef> {
        let mut children = self.children.write();
        if children.contains_key(name) {
            bail!(EEXIST);
        }
        let link: InodeRef = RamSymlink::new(target);
        children.insert(name.to_string(), link.clone());
        Ok(link)
    }

    impl_as_any!();
}

/// A symbolic link.
pub struct RamSymlink {
    ino: u64,
    target: String,
}

impl RamSymlink {
    pub fn new(target: &str) -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            target: target.to_string(),
        })
    }
}

impl Inode for RamSymlink {
    fn kind(&self) -> InodeKind {
        InodeKind::Symlink
    }

    fn ino(&self) -> u64 {
        self.ino
    }

    fn size(&self) -> usize {
        self.target.len()
    }

    fn mode(&self) -> u32 {
        0o777
    }

    fn readlink(&self) -> Result<String> {
        Ok(self.target.clone())
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let data = self.target.as_bytes();
        if offset >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - offset);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        Ok(n)
    }

    impl_as_any!();
}
