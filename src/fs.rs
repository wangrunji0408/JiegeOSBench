//! In-memory filesystem (ramfs) with a Unix-like VFS for the kernel.

use crate::sync::SpinLock;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// A node in the in-memory filesystem (file or directory).
pub struct INode {
    pub name: String,
    pub is_dir: bool,
    /// File content (valid for regular files).
    pub data: SpinLock<Vec<u8>>,
    /// Directory entries (valid for directories).
    pub children: SpinLock<Vec<Arc<INode>>>,
}

impl INode {
    pub fn new_file(name: &str, data: Vec<u8>) -> Arc<INode> {
        Arc::new(INode {
            name: String::from(name),
            is_dir: false,
            data: SpinLock::new(data),
            children: SpinLock::new(Vec::new()),
        })
    }

    pub fn new_dir(name: &str) -> Arc<INode> {
        Arc::new(INode {
            name: String::from(name),
            is_dir: true,
            data: SpinLock::new(Vec::new()),
            children: SpinLock::new(Vec::new()),
        })
    }

    pub fn add_child(self: &Arc<Self>, child: Arc<INode>) {
        self.children.lock().push(child);
    }

    pub fn lookup(self: &Arc<Self>, name: &str) -> Option<Arc<INode>> {
        if !self.is_dir {
            return None;
        }
        self.children.lock().iter().find(|c| c.name == name).cloned()
    }
}

/// The kind of open file a descriptor refers to.
pub enum FileKind {
    /// /dev/null
    Null,
    Stdin,
    Stdout,
    Stderr,
    /// A regular file or directory.
    Inode(Arc<INode>),
    /// A networking socket (implemented later).
    Socket(usize),
}

pub struct FileDesc {
    pub kind: FileKind,
    pub offset: usize,
    pub flags: u32,
    pub readable: bool,
    pub writable: bool,
}

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_CLOEXEC: u32 = 0o2000000;

static ROOT: SpinLock<Option<Arc<INode>>> = SpinLock::new(None);

/// Populate the filesystem with the layout nginx expects.
pub fn init() {
    let root = INode::new_dir("/");
    let dev = INode::new_dir("dev");
    let tmp = INode::new_dir("tmp");
    let run = INode::new_dir("run");
    let www = INode::new_dir("www");
    let usr = INode::new_dir("usr");
    let local = INode::new_dir("local");
    let nginx = INode::new_dir("nginx");
    let conf = INode::new_dir("conf");
    let sbin = INode::new_dir("sbin");

    conf.add_child(INode::new_file("mime.types", include_bytes!("../../nginx-conf/mime.types").to_vec()));
    conf.add_child(INode::new_file("nginx.conf", include_bytes!("../../nginx-conf/nginx.conf").to_vec()));
    sbin.add_child(INode::new_file("nginx", include_bytes!("../../third_party/nginx").to_vec()));
    nginx.add_child(conf);
    nginx.add_child(sbin);
    local.add_child(nginx);
    usr.add_child(local);
    www.add_child(INode::new_file("index.html", include_bytes!("../../webroot/index.html").to_vec()));

    root.add_child(dev);
    root.add_child(tmp);
    root.add_child(run);
    root.add_child(www);
    root.add_child(usr);

    *ROOT.lock() = Some(root);
}

fn root() -> Arc<INode> {
    ROOT.lock().as_ref().expect("fs not initialized").clone()
}

/// Normalize and split a path into components (without leading/trailing slashes).
fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty() && *s != ".").collect()
}

/// Look up a node by absolute path. Returns None if not found.
pub fn lookup(path: &str) -> Option<Arc<INode>> {
    let mut cur = root();
    for comp in split_path(path) {
        if comp == ".." {
            // we don't track parent links; treat as staying in place
            continue;
        }
        match cur.lookup(comp) {
            Some(n) => cur = n,
            None => return None,
        }
    }
    Some(cur)
}

/// Look up the parent directory and the final component name.
pub fn lookup_parent(path: &str) -> Option<(Arc<INode>, String)> {
    let comps = split_path(path);
    if comps.is_empty() {
        return None;
    }
    let (last, parents) = comps.split_last().unwrap();
    let mut cur = root();
    for comp in parents {
        if *comp == ".." {
            continue;
        }
        match cur.lookup(comp) {
            Some(n) => cur = n,
            None => return None,
        }
    }
    Some((cur, String::from(*last)))
}

/// Create (or truncate) a file at `path`, returning the inode.
pub fn create_file(path: &str) -> Option<Arc<INode>> {
    let (dir, name) = lookup_parent(path)?;
    if !dir.is_dir {
        return None;
    }
    if let Some(existing) = dir.lookup(&name) {
        if existing.is_dir {
            return None;
        }
        existing.data.lock().clear();
        return Some(existing);
    }
    let node = INode::new_file(&name, Vec::new());
    dir.add_child(node.clone());
    Some(node)
}

pub fn mkdir(path: &str) -> isize {
    let (dir, name) = lookup_parent(path).ok_or(-1isize).unwrap_or_else(|_| {
        // fall back: -1
        // (placeholder; replaced below)
        let _ = &dir;
        let _ = &name;
        unreachable!()
    });
    if !dir.is_dir {
        return -crate::syscall::ENOTDIR;
    }
    if dir.lookup(&name).is_some() {
        return -crate::syscall::EEXIST;
    }
    dir.add_child(INode::new_dir(&name));
    0
}

pub fn unlink(path: &str) -> isize {
    let (dir, name) = match lookup_parent(path) {
        Some(x) => x,
        None => return -crate::syscall::ENOENT,
    };
    let mut children = dir.children.lock();
    if let Some(pos) = children.iter().position(|c| c.name == name) {
        children.remove(pos);
        0
    } else {
        -crate::syscall::ENOENT
    }
}

/// Fill a Linux riscv64 `struct stat` (128 bytes) into `out`.
pub fn stat_of(node: &Arc<INode>, out: &mut [u8; 128]) {
    out.fill(0);
    let mut w = |off: usize, bytes: &[u8]| out[off..off + bytes.len()].copy_from_slice(bytes);

    let (mode, size) = {
        let data = node.data.lock();
        let size = data.len() as u64;
        let mode = if node.is_dir { 0o040755u32 } else { 0o100644u32 };
        (mode, size)
    };

    w(0, &0u64.to_le_bytes());            // st_dev
    w(8, &(1u64).to_le_bytes());          // st_ino
    w(16, &mode.to_le_bytes());           // st_mode
    w(20, &1u32.to_le_bytes());           // st_nlink
    w(24, &0u32.to_le_bytes());           // st_uid
    w(28, &0u32.to_le_bytes());           // st_gid
    w(32, &0u64.to_le_bytes());           // st_rdev
    w(48, &(size as i64).to_le_bytes());  // st_size
    w(56, &4096i32.to_le_bytes());        // st_blksize
    w(64, &((size / 512) as i64).to_le_bytes()); // st_blocks
    let t = crate::sbi::get_time() as i64;
    w(72, &t.to_le_bytes());              // st_atime
    w(88, &t.to_le_bytes());              // st_mtime
    w(104, &t.to_le_bytes());             // st_ctime
}
