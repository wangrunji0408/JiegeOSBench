/// 内存文件系统（RamFS）
/// 存储所有文件，包括从initramfs解包的nginx

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use lazy_static::lazy_static;

use super::{FileType, FileStat};

#[derive(Clone)]
pub struct INode {
    pub kind: INodeKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub ino: u64,
}

#[derive(Clone)]
pub enum INodeKind {
    File(Arc<Mutex<Vec<u8>>>),
    Dir(Arc<Mutex<BTreeMap<String, Arc<Mutex<INode>>>>>),
    Symlink(String),
    CharDev { major: u32, minor: u32 },
    BlockDev { major: u32, minor: u32 },
    Fifo,
    Socket,
}

impl INode {
    pub fn new_file(mode: u32) -> Arc<Mutex<INode>> {
        static INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
        Arc::new(Mutex::new(INode {
            kind: INodeKind::File(Arc::new(Mutex::new(Vec::new()))),
            mode,
            uid: 0,
            gid: 0,
            ino: INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        }))
    }

    pub fn new_dir(mode: u32) -> Arc<Mutex<INode>> {
        static INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1000);
        Arc::new(Mutex::new(INode {
            kind: INodeKind::Dir(Arc::new(Mutex::new(BTreeMap::new()))),
            mode,
            uid: 0,
            gid: 0,
            ino: INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        }))
    }

    pub fn new_symlink(target: String, mode: u32) -> Arc<Mutex<INode>> {
        static INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(2000);
        Arc::new(Mutex::new(INode {
            kind: INodeKind::Symlink(target),
            mode,
            uid: 0,
            gid: 0,
            ino: INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        }))
    }

    pub fn new_char_dev(major: u32, minor: u32) -> Arc<Mutex<INode>> {
        static INO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(3000);
        Arc::new(Mutex::new(INode {
            kind: INodeKind::CharDev { major, minor },
            mode: 0o020666,
            uid: 0,
            gid: 0,
            ino: INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        }))
    }

    pub fn stat(&self) -> FileStat {
        let (file_type, size, rdev) = match &self.kind {
            INodeKind::File(data) => (FileType::Regular, data.lock().len(), 0u64),
            INodeKind::Dir(_) => (FileType::Directory, 0, 0),
            INodeKind::Symlink(t) => (FileType::Symlink, t.len(), 0),
            INodeKind::CharDev { major, minor } => {
                (FileType::CharDevice, 0, ((*major as u64) << 8) | (*minor as u64))
            }
            INodeKind::BlockDev { major, minor } => {
                (FileType::BlockDevice, 0, ((*major as u64) << 8) | (*minor as u64))
            }
            INodeKind::Fifo => (FileType::Fifo, 0, 0),
            INodeKind::Socket => (FileType::Socket, 0, 0),
        };
        FileStat {
            size,
            file_type,
            mode: self.mode,
            uid: self.uid,
            gid: self.gid,
            nlink: 1,
            ino: self.ino,
            rdev,
        }
    }
}

pub struct FileSystem {
    root: Arc<Mutex<INode>>,
}

unsafe impl Send for FileSystem {}
unsafe impl Sync for FileSystem {}

impl FileSystem {
    pub fn new() -> Self {
        Self {
            root: INode::new_dir(0o755),
        }
    }

    pub fn root(&self) -> Arc<Mutex<INode>> {
        self.root.clone()
    }

    /// 创建目录（递归）
    pub fn mkdir_p(&self, path: &str) {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = self.root.clone();
        for part in parts {
            let next = {
                let node = current.lock();
                if let INodeKind::Dir(entries) = &node.kind {
                    let mut map = entries.lock();
                    if !map.contains_key(part) {
                        let new_dir = INode::new_dir(0o755);
                        map.insert(part.to_string(), new_dir.clone());
                        new_dir
                    } else {
                        map[part].clone()
                    }
                } else {
                    panic!("{} is not a directory", part);
                }
            };
            current = next;
        }
    }

    /// 创建文件
    pub fn create_file(&self, path: &str, data: Vec<u8>, mode: u32) {
        let (parent_path, name) = split_path(path);
        self.mkdir_p(parent_path);
        let parent = self.lookup(parent_path).expect("parent not found");
        let file = INode::new_file(mode);
        {
            let f = file.lock();
            if let INodeKind::File(content) = &f.kind {
                *content.lock() = data;
            }
        }
        let parent_guard = parent.lock();
        if let INodeKind::Dir(entries) = &parent_guard.kind {
            entries.lock().insert(name.to_string(), file);
        }
    }

    /// 创建符号链接
    pub fn create_symlink(&self, path: &str, target: &str, mode: u32) {
        let (parent_path, name) = split_path(path);
        self.mkdir_p(parent_path);
        let parent = self.lookup(parent_path).expect("parent not found");
        let link = INode::new_symlink(target.to_string(), mode);
        let parent_guard = parent.lock();
        if let INodeKind::Dir(entries) = &parent_guard.kind {
            entries.lock().insert(name.to_string(), link);
        }
    }

    /// 创建字符设备
    pub fn create_char_dev(&self, path: &str, major: u32, minor: u32) {
        let (parent_path, name) = split_path(path);
        self.mkdir_p(parent_path);
        let parent = self.lookup(parent_path).expect("parent not found");
        let dev = INode::new_char_dev(major, minor);
        let parent_guard = parent.lock();
        if let INodeKind::Dir(entries) = &parent_guard.kind {
            entries.lock().insert(name.to_string(), dev);
        }
    }

    /// 查找路径（解析符号链接）
    pub fn lookup(&self, path: &str) -> Option<Arc<Mutex<INode>>> {
        self.lookup_impl(path, 0)
    }

    fn lookup_impl(&self, path: &str, depth: usize) -> Option<Arc<Mutex<INode>>> {
        if depth > 10 { return None; } // 防止符号链接循环
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = self.root.clone();

        for part in &parts {
            let next = {
                let node = current.lock();
                match &node.kind {
                    INodeKind::Dir(entries) => {
                        entries.lock().get(*part)?.clone()
                    }
                    INodeKind::Symlink(target) => {
                        // 解析符号链接
                        let resolved = if target.starts_with('/') {
                            target.clone()
                        } else {
                            // 相对路径
                            let parent = parts[..parts.len()-1].join("/");
                            format!("/{}/{}", parent, target)
                        };
                        return self.lookup_impl(&resolved, depth + 1);
                    }
                    _ => return None,
                }
            };

            // 如果找到的是符号链接，解析它
            let resolved = {
                let node = next.lock();
                if let INodeKind::Symlink(target) = &node.kind {
                    Some(target.clone())
                } else {
                    None
                }
            };

            if let Some(target) = resolved {
                let resolved_path = if target.starts_with('/') {
                    target
                } else {
                    let current_path = parts[..parts.iter().position(|&p| p == *part).unwrap() + 1].join("/");
                    let parent_path = parts[..parts.iter().position(|&p| p == *part).unwrap()].join("/");
                    format!("/{}/{}", parent_path, target)
                };
                current = self.lookup_impl(&resolved_path, depth + 1)?;
            } else {
                current = next;
            }
        }

        Some(current)
    }

    /// 列出目录内容
    pub fn readdir(&self, path: &str) -> Option<Vec<(String, Arc<Mutex<INode>>)>> {
        let node = self.lookup(path)?;
        let node = node.lock();
        if let INodeKind::Dir(entries) = &node.kind {
            let map = entries.lock();
            Some(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        } else {
            None
        }
    }
}

fn split_path(path: &str) -> (&str, &str) {
    let path = path.trim_end_matches('/');
    if let Some(pos) = path.rfind('/') {
        let parent = &path[..pos];
        let name = &path[pos+1..];
        if parent.is_empty() { ("/", name) } else { (parent, name) }
    } else {
        ("/", path)
    }
}

lazy_static! {
    pub static ref FS: FileSystem = FileSystem::new();
}

pub fn init() {
    // 创建基础目录结构
    FS.mkdir_p("/proc");
    FS.mkdir_p("/sys");
    FS.mkdir_p("/dev");
    FS.mkdir_p("/tmp");
    FS.mkdir_p("/run");
    FS.mkdir_p("/var/log/nginx");
    FS.mkdir_p("/var/run");
    FS.mkdir_p("/var/www/html");
    FS.mkdir_p("/var/lib/nginx/body");
    FS.mkdir_p("/var/lib/nginx/fastcgi");
    FS.mkdir_p("/var/lib/nginx/proxy");
    FS.mkdir_p("/var/lib/nginx/scgi");
    FS.mkdir_p("/var/lib/nginx/uwsgi");
    FS.mkdir_p("/etc");
    FS.mkdir_p("/usr/lib");
    FS.mkdir_p("/usr/share");

    // 创建设备文件
    FS.create_char_dev("/dev/null", 1, 3);
    FS.create_char_dev("/dev/zero", 1, 5);
    FS.create_char_dev("/dev/random", 1, 8);
    FS.create_char_dev("/dev/urandom", 1, 9);
    FS.create_char_dev("/dev/tty", 5, 0);
    FS.create_char_dev("/dev/console", 5, 1);
    // nginx/syslog用的socket文件（空文件模拟）
    FS.create_file("/dev/log", alloc::vec![], 0o666);
    FS.create_file("/var/run/syslog", alloc::vec![], 0o666);

    // /proc/self/fd
    FS.mkdir_p("/proc/self/fd");
    FS.mkdir_p("/proc/self");

    // 基本的/etc/passwd
    FS.create_file("/etc/passwd", b"root:x:0:0:root:/root:/bin/sh\n".to_vec(), 0o644);
    FS.create_file("/etc/group", b"root:x:0:\n".to_vec(), 0o644);
    FS.create_file("/etc/hosts", b"127.0.0.1 localhost\n".to_vec(), 0o644);

    println!("[fs] RamFS initialized");
}
