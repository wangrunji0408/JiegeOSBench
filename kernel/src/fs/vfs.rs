//! In-memory filesystem tree (ramfs) with path resolution.
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::abi::*;
use crate::sync::{Global, SpinLock};

static NEXT_INO: AtomicU64 = AtomicU64::new(2);
pub static ROOT: Global<Arc<Dentry>> = Global::new();

#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub rdev: u64,
}

pub enum NodeKind {
    File(SpinLock<Vec<u8>>),
    Dir(SpinLock<BTreeMap<String, Arc<Dentry>>>),
    Symlink(String),
    /// Character device identified by (major, minor).
    CharDev(u32, u32),
    Fifo,
    Socket,
}

pub struct Dentry {
    pub ino: u64,
    pub name: SpinLock<String>,
    pub parent: SpinLock<Weak<Dentry>>,
    pub meta: SpinLock<Meta>,
    pub kind: NodeKind,
    /// Wait queue used by fifos/sockets bound in the tree (unused for files).
    pub gen: AtomicU64,
}

pub fn now_secs() -> i64 {
    (crate::time::realtime_ns() / 1_000_000_000) as i64
}

impl Dentry {
    pub fn new(name: &str, parent: Weak<Dentry>, mode: u32, kind: NodeKind) -> Arc<Dentry> {
        let t = now_secs();
        Arc::new(Dentry {
            ino: NEXT_INO.fetch_add(1, Ordering::Relaxed),
            name: SpinLock::new(String::from(name)),
            parent: SpinLock::new(parent),
            meta: SpinLock::new(Meta { mode, uid: 0, gid: 0, nlink: 1, atime: t, mtime: t, ctime: t, rdev: 0 }),
            kind,
            gen: AtomicU64::new(0),
        })
    }

    pub fn new_dir(name: &str, parent: Weak<Dentry>, mode: u32) -> Arc<Dentry> {
        Self::new(name, parent, S_IFDIR | (mode & 0o7777), NodeKind::Dir(SpinLock::new(BTreeMap::new())))
    }

    pub fn new_file(name: &str, parent: Weak<Dentry>, mode: u32, data: Vec<u8>) -> Arc<Dentry> {
        Self::new(name, parent, S_IFREG | (mode & 0o7777), NodeKind::File(SpinLock::new(data)))
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Dir(_))
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, NodeKind::Symlink(_))
    }

    pub fn is_file(&self) -> bool {
        matches!(self.kind, NodeKind::File(_))
    }

    pub fn mode(&self) -> u32 {
        self.meta.lock().mode
    }

    pub fn file_type(&self) -> u32 {
        self.mode() & S_IFMT
    }

    pub fn size(&self) -> u64 {
        match &self.kind {
            NodeKind::File(d) => d.lock().len() as u64,
            NodeKind::Symlink(t) => t.len() as u64,
            NodeKind::Dir(_) => 4096,
            _ => 0,
        }
    }

    pub fn parent(&self) -> Option<Arc<Dentry>> {
        self.parent.lock().upgrade()
    }

    pub fn lookup_child(&self, name: &str) -> Option<Arc<Dentry>> {
        match &self.kind {
            NodeKind::Dir(c) => c.lock().get(name).cloned(),
            _ => None,
        }
    }

    pub fn children(&self) -> Vec<(String, Arc<Dentry>)> {
        match &self.kind {
            NodeKind::Dir(c) => c.lock().iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            _ => Vec::new(),
        }
    }

    pub fn add_child(self: &Arc<Self>, child: Arc<Dentry>) -> Result<(), i32> {
        match &self.kind {
            NodeKind::Dir(c) => {
                let mut c = c.lock();
                let name = child.name.lock().clone();
                if c.contains_key(&name) {
                    return Err(EEXIST);
                }
                *child.parent.lock() = Arc::downgrade(self);
                c.insert(name, child);
                let mut m = self.meta.lock();
                m.mtime = now_secs();
                m.ctime = m.mtime;
                Ok(())
            }
            _ => Err(ENOTDIR),
        }
    }

    pub fn remove_child(&self, name: &str) -> Result<Arc<Dentry>, i32> {
        match &self.kind {
            NodeKind::Dir(c) => {
                let mut c = c.lock();
                let d = c.remove(name).ok_or(ENOENT)?;
                let mut m = self.meta.lock();
                m.mtime = now_secs();
                m.ctime = m.mtime;
                Ok(d)
            }
            _ => Err(ENOTDIR),
        }
    }

    /// Full path of this node.
    pub fn path(self: &Arc<Self>) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = self.clone();
        loop {
            let parent = cur.parent();
            match parent {
                Some(p) => {
                    parts.push(cur.name.lock().clone());
                    cur = p;
                }
                None => break,
            }
        }
        if parts.is_empty() {
            return String::from("/");
        }
        let mut s = String::new();
        for p in parts.iter().rev() {
            s.push('/');
            s.push_str(p);
        }
        s
    }

    pub fn stat(&self) -> Stat {
        let m = *self.meta.lock();
        let size = self.size();
        Stat {
            st_dev: 1,
            st_ino: self.ino,
            st_mode: m.mode,
            st_nlink: m.nlink,
            st_uid: m.uid,
            st_gid: m.gid,
            st_rdev: m.rdev,
            __pad1: 0,
            st_size: size as i64,
            st_blksize: 4096,
            __pad2: 0,
            st_blocks: ((size + 511) / 512) as i64,
            st_atime: m.atime,
            st_atime_nsec: 0,
            st_mtime: m.mtime,
            st_mtime_nsec: 0,
            st_ctime: m.ctime,
            st_ctime_nsec: 0,
            __unused: [0; 2],
        }
    }

    pub fn touch(&self) {
        let mut m = self.meta.lock();
        m.mtime = now_secs();
        m.ctime = m.mtime;
    }
}

pub fn root() -> Arc<Dentry> {
    ROOT.get().clone()
}

pub fn init_root() {
    let root = Dentry::new_dir("", Weak::new(), 0o755);
    ROOT.init(root);
}

const MAX_SYMLINKS: usize = 40;

/// Resolve `path` relative to `base`. If `follow_last` is false, a trailing
/// symlink is returned itself.
pub fn lookup(base: &Arc<Dentry>, path: &str, follow_last: bool) -> Result<Arc<Dentry>, i32> {
    let mut depth = 0;
    lookup_inner(base, path, follow_last, &mut depth)
}

fn lookup_inner(base: &Arc<Dentry>, path: &str, follow_last: bool, depth: &mut usize) -> Result<Arc<Dentry>, i32> {
    let mut cur = if path.starts_with('/') { root() } else { base.clone() };
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let n = comps.len();
    for (i, comp) in comps.iter().enumerate() {
        let last = i + 1 == n;
        if !cur.is_dir() {
            return Err(ENOTDIR);
        }
        let next = match *comp {
            "." => cur.clone(),
            ".." => cur.parent().unwrap_or_else(root),
            name => cur.lookup_child(name).ok_or(ENOENT)?,
        };
        if let NodeKind::Symlink(target) = &next.kind {
            if !last || follow_last {
                *depth += 1;
                if *depth > MAX_SYMLINKS {
                    return Err(ELOOP);
                }
                let resolved = lookup_inner(&cur, target, true, depth)?;
                cur = resolved;
                continue;
            }
        }
        cur = next;
    }
    Ok(cur)
}

/// Resolve everything but the last component; returns (parent dir, last name).
pub fn lookup_parent(base: &Arc<Dentry>, path: &str) -> Result<(Arc<Dentry>, String), i32> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        if path.starts_with('/') {
            return Err(EEXIST);
        }
        return Err(ENOENT);
    }
    let (dir, name) = match trimmed.rfind('/') {
        Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
        None => ("", trimmed),
    };
    let parent = if dir.is_empty() {
        if trimmed.starts_with('/') {
            root()
        } else {
            base.clone()
        }
    } else {
        lookup(base, dir, true)?
    };
    if !parent.is_dir() {
        return Err(ENOTDIR);
    }
    if name == "." || name == ".." {
        return Err(EINVAL);
    }
    Ok((parent, String::from(name)))
}

/// Create intermediate directories (used at boot).
pub fn mkdir_p(path: &str) -> Arc<Dentry> {
    let mut cur = root();
    for comp in path.split('/').filter(|c| !c.is_empty()) {
        let next = match cur.lookup_child(comp) {
            Some(d) => d,
            None => {
                let d = Dentry::new_dir(comp, Arc::downgrade(&cur), 0o755);
                cur.add_child(d.clone()).unwrap();
                d
            }
        };
        cur = next;
    }
    cur
}

pub fn create_file(path: &str, mode: u32, data: Vec<u8>) -> Result<Arc<Dentry>, i32> {
    let (parent, name) = lookup_parent(&root(), path)?;
    let f = Dentry::new_file(&name, Arc::downgrade(&parent), mode, data);
    parent.add_child(f.clone())?;
    Ok(f)
}

pub fn create_node(path: &str, mode: u32, kind: NodeKind) -> Result<Arc<Dentry>, i32> {
    let (parent, name) = lookup_parent(&root(), path)?;
    let f = Dentry::new(&name, Arc::downgrade(&parent), mode, kind);
    parent.add_child(f.clone())?;
    Ok(f)
}
