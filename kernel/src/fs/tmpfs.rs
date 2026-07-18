//! An in-memory filesystem tree, populated at boot from an embedded tar
//! archive (see `tar.rs`). Not persisted across reboots -- fine for a demo
//! kernel whose whole rootfs is baked into the kernel image anyway.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, Once};

pub enum Inode {
    File(Mutex<Vec<u8>>),
    Dir(Mutex<BTreeMap<String, Arc<Inode>>>),
    Symlink(String),
}

impl Inode {
    pub fn new_dir() -> Arc<Inode> {
        Arc::new(Inode::Dir(Mutex::new(BTreeMap::new())))
    }
    pub fn new_file(data: Vec<u8>) -> Arc<Inode> {
        Arc::new(Inode::File(Mutex::new(data)))
    }
    pub fn is_dir(&self) -> bool {
        matches!(self, Inode::Dir(_))
    }
    pub fn size(&self) -> usize {
        match self {
            Inode::File(d) => d.lock().len(),
            _ => 0,
        }
    }
}

static ROOT: Once<Arc<Inode>> = Once::new();

pub fn root() -> Arc<Inode> {
    ROOT.get().unwrap().clone()
}

pub fn init() {
    ROOT.call_once(Inode::new_dir);
    super::tar::extract(include_bytes!("rootfs/rootfs.tar"), &root());
}

/// Create (or reuse) the directory named by an absolute path, creating any
/// missing intermediate components. Used both by the tar extractor and by
/// `mkdirat`.
pub fn make_dirs_absolute(path: &str) -> Option<Arc<Inode>> {
    let mut cur = root();
    for comp in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
        let next = {
            let dir = match &*cur {
                Inode::Dir(m) => m,
                _ => return None,
            };
            let mut dir = dir.lock();
            dir.entry(comp.to_string())
                .or_insert_with(Inode::new_dir)
                .clone()
        };
        if !next.is_dir() {
            return None;
        }
        cur = next;
    }
    Some(cur)
}

/// Insert a file (or symlink) at an absolute path, creating parent
/// directories as needed.
pub fn insert_absolute(path: &str, inode: Arc<Inode>) -> bool {
    let (parent, name) = match path.rsplit_once('/') {
        Some((p, n)) => (p, n),
        None => ("", path),
    };
    let parent_dir = if parent.is_empty() {
        root()
    } else {
        match make_dirs_absolute(parent) {
            Some(d) => d,
            None => return false,
        }
    };
    match &*parent_dir {
        Inode::Dir(m) => {
            m.lock().insert(name.to_string(), inode);
            true
        }
        _ => false,
    }
}

/// Resolve an absolute path to an inode, following symlinks (bounded
/// depth). Relative paths are treated as relative to `/` -- every path
/// nginx itself uses is absolute, so this is not a limitation in practice.
pub fn resolve(path: &str) -> Option<Arc<Inode>> {
    let mut cur = root();
    let mut work: Vec<String> = split_components(path);
    work.reverse();
    let mut depth = 0;
    while let Some(comp) = work.pop() {
        if comp == ".." {
            continue;
        }
        let dir = match &*cur {
            Inode::Dir(m) => m,
            _ => return None,
        };
        let next = dir.lock().get(&comp)?.clone();
        if let Inode::Symlink(target) = &*next {
            depth += 1;
            if depth > 16 {
                return None;
            }
            let mut target_parts = split_components(target);
            if target.starts_with('/') {
                cur = root();
            }
            target_parts.reverse();
            work.extend(target_parts);
        } else {
            cur = next;
        }
    }
    Some(cur)
}

fn split_components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(String::from)
        .collect()
}

/// Resolve the parent directory of an absolute path plus the final
/// component's name, e.g. for creating a new entry there (`openat` with
/// `O_CREAT`, `mkdirat`).
pub fn resolve_parent(path: &str) -> Option<(Arc<Inode>, String)> {
    let (parent, name) = match path.rsplit_once('/') {
        Some((p, n)) => (p, n),
        None => ("", path),
    };
    let parent_dir = if parent.is_empty() {
        root()
    } else {
        resolve(parent)?
    };
    if !parent_dir.is_dir() {
        return None;
    }
    Some((parent_dir, name.to_string()))
}
