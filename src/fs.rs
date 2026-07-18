//! 内存文件系统（ramfs）+ 启动时从内置 tar 解包

use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use lazy_static::lazy_static;

pub const S_IFREG: u32 = 0o100000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFCHR: u32 = 0o020000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    None,
    Null,
    Zero,
    Urandom,
}

pub struct Node {
    pub name: String,
    pub parent: usize,
    pub mode: u32, // 含类型位
    pub children: BTreeMap<String, usize>,
    pub data: Vec<u8>,
    pub link_target: String,
    pub special: Special,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }
    pub fn size(&self) -> usize {
        if self.is_symlink() {
            self.link_target.len()
        } else {
            self.data.len()
        }
    }
}

const S_IFMT: u32 = 0o170000;

pub struct RamFs {
    pub nodes: Vec<Node>,
}

pub const ENOENT: i32 = -2;
pub const EEXIST: i32 = -17;
pub const ENOTDIR: i32 = -20;
pub const EISDIR: i32 = -21;
pub const ELOOP: i32 = -40;
pub const ENOTEMPTY: i32 = -39;

impl RamFs {
    fn new() -> Self {
        let mut fs = Self { nodes: Vec::new() };
        fs.nodes.push(Node {
            name: String::from("/"),
            parent: 0,
            mode: S_IFDIR | 0o755,
            children: BTreeMap::new(),
            data: Vec::new(),
            link_target: String::new(),
            special: Special::None,
        });
        fs
    }

    fn new_node(&mut self, name: &str, parent: usize, mode: u32) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: String::from(name),
            parent,
            mode,
            children: BTreeMap::new(),
            data: Vec::new(),
            link_target: String::new(),
            special: Special::None,
        });
        self.nodes[parent]
            .children
            .insert(String::from(name), id);
        id
    }

    /// 逐级创建目录（绝对路径）
    pub fn mkdir_p(&mut self, path: &str) -> usize {
        let mut cur = 0usize;
        for comp in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            if comp == ".." {
                cur = self.nodes[cur].parent;
                continue;
            }
            match self.nodes[cur].children.get(comp) {
                Some(&id) => cur = id,
                None => {
                    cur = self.new_node(comp, cur, S_IFDIR | 0o755);
                }
            }
        }
        cur
    }

    /// 解析路径，返回节点 id。follow_final: 是否跟随最后一个符号链接
    pub fn lookup(&self, path: &str, cwd: &str, follow_final: bool) -> Result<usize, i32> {
        self.resolve(path, cwd, follow_final, 0)
    }

    fn resolve(
        &self,
        path: &str,
        cwd: &str,
        follow_final: bool,
        depth: u32,
    ) -> Result<usize, i32> {
        if depth > 8 {
            return Err(ELOOP);
        }
        let mut cur = if path.starts_with('/') {
            0usize
        } else {
            self.lookup_dir(cwd)?
        };
        let comps: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for (i, comp) in comps.iter().enumerate() {
            let last = i == comps.len() - 1;
            let node = &self.nodes[cur];
            if *comp == "." {
                continue;
            }
            if *comp == ".." {
                cur = node.parent;
                continue;
            }
            if !node.is_dir() {
                return Err(ENOTDIR);
            }
            let &next = node.children.get(*comp).ok_or(ENOENT)?;
            let next_node = &self.nodes[next];
            if next_node.is_symlink() && (!last || follow_final) {
                // 跟随符号链接
                let target = next_node.link_target.clone();
                let base_dir = if target.starts_with('/') {
                    String::from("/")
                } else {
                    self.path_of(cur)
                };
                let resolved = self.resolve(&target, &base_dir, true, depth + 1)?;
                cur = resolved;
            } else {
                cur = next;
            }
        }
        Ok(cur)
    }

    fn lookup_dir(&self, cwd: &str) -> Result<usize, i32> {
        // cwd 总是绝对路径且为目录
        let mut cur = 0usize;
        for comp in cwd.split('/').filter(|s| !s.is_empty()) {
            let node = &self.nodes[cur];
            let &next = node.children.get(comp).ok_or(ENOENT)?;
            cur = next;
        }
        Ok(cur)
    }

    pub fn path_of(&self, mut id: usize) -> String {
        if id == 0 {
            return String::from("/");
        }
        let mut parts = Vec::new();
        while id != 0 {
            parts.push(self.nodes[id].name.clone());
            id = self.nodes[id].parent;
        }
        let mut s = String::from("/");
        for (i, p) in parts.iter().rev().enumerate() {
            if i > 0 {
                s.push('/');
            }
            s.push_str(p);
        }
        s
    }

    /// 解析父目录与最后一级名称
    pub fn lookup_parent(&self, path: &str, cwd: &str) -> Result<(usize, String), i32> {
        let trimmed = path.trim_end_matches('/');
        let (dir_path, name) = match trimmed.rfind('/') {
            Some(0) => ("/", &trimmed[1..]),
            Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
            None => (".", trimmed),
        };
        if name.is_empty() || name == "." || name == ".." {
            return Err(EEXIST);
        }
        let dir = self.resolve(dir_path, cwd, true, 0)?;
        Ok((dir, String::from(name)))
    }

    pub fn create_file(&mut self, path: &str, cwd: &str, mode: u32) -> Result<usize, i32> {
        let (dir, name) = self.lookup_parent(path, cwd)?;
        if let Some(&id) = self.nodes[dir].children.get(&name) {
            return Ok(id);
        }
        Ok(self.new_node(&name, dir, S_IFREG | (mode & 0o777)))
    }

    pub fn create_symlink(&mut self, path: &str, cwd: &str, target: &str) -> Result<usize, i32> {
        let (dir, name) = self.lookup_parent(path, cwd)?;
        if self.nodes[dir].children.contains_key(&name) {
            return Err(EEXIST);
        }
        let id = self.new_node(&name, dir, S_IFLNK | 0o777);
        self.nodes[id].link_target = String::from(target);
        Ok(id)
    }

    pub fn mkdir(&mut self, path: &str, cwd: &str, mode: u32) -> Result<usize, i32> {
        let (dir, name) = self.lookup_parent(path, cwd)?;
        if self.nodes[dir].children.contains_key(&name) {
            return Err(EEXIST);
        }
        Ok(self.new_node(&name, dir, S_IFDIR | (mode & 0o777)))
    }

    pub fn unlink(&mut self, path: &str, cwd: &str) -> Result<(), i32> {
        let (dir, name) = self.lookup_parent(path, cwd)?;
        let &id = self.nodes[dir].children.get(&name).ok_or(ENOENT)?;
        if self.nodes[id].is_dir() {
            return Err(EISDIR);
        }
        self.nodes[dir].children.remove(&name);
        Ok(())
    }

    pub fn rmdir(&mut self, path: &str, cwd: &str) -> Result<(), i32> {
        let (dir, name) = self.lookup_parent(path, cwd)?;
        let &id = self.nodes[dir].children.get(&name).ok_or(ENOENT)?;
        if !self.nodes[id].is_dir() {
            return Err(ENOTDIR);
        }
        if !self.nodes[id].children.is_empty() {
            return Err(ENOTEMPTY);
        }
        self.nodes[dir].children.remove(&name);
        Ok(())
    }

    pub fn read(&self, id: usize, offset: usize, buf: &mut [u8]) -> usize {
        let node = &self.nodes[id];
        if offset >= node.data.len() {
            return 0;
        }
        let len = core::cmp::min(buf.len(), node.data.len() - offset);
        buf[..len].copy_from_slice(&node.data[offset..offset + len]);
        len
    }

    /// 写入，返回写入长度
    pub fn write(&mut self, id: usize, offset: usize, data: &[u8]) -> usize {
        let node = &mut self.nodes[id];
        let end = offset + data.len();
        if end > node.data.len() {
            node.data.resize(end, 0);
        }
        node.data[offset..end].copy_from_slice(data);
        data.len()
    }

    pub fn truncate(&mut self, id: usize, len: usize) {
        self.nodes[id].data.resize(len, 0);
    }

    pub fn readdir(&self, id: usize) -> Vec<(String, u32)> {
        let node = &self.nodes[id];
        let mut out = Vec::new();
        out.push((String::from("."), S_IFDIR));
        out.push((String::from(".."), S_IFDIR));
        for (name, &cid) in node.children.iter() {
            out.push((name.clone(), self.nodes[cid].mode & S_IFMT));
        }
        out
    }
}

lazy_static! {
    static ref RAMFS: UPIntrFreeCell<RamFs> = unsafe { UPIntrFreeCell::new(RamFs::new()) };
}

pub fn with_fs<R>(f: impl FnOnce(&mut RamFs) -> R) -> R {
    let mut guard = RAMFS.lock();
    f(&mut guard)
}

/// 内嵌的 rootfs tar 包
static ROOTFS_TAR: &[u8] = include_bytes!("../rootfs.tar");

pub fn tar_checksum() -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in ROOTFS_TAR.iter() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn init() {
    with_fs(|fs| {
        unpack_tar(fs, ROOTFS_TAR);
        // 设备文件与运行目录
        fs.mkdir_p("/dev");
        fs.mkdir_p("/tmp");
        fs.mkdir_p("/run");
        fs.mkdir_p("/proc");
        fs.mkdir_p("/var/log/nginx");
        fs.mkdir_p("/var/lib/nginx/tmp/client_body");
        fs.mkdir_p("/var/lib/nginx/tmp/proxy");
        fs.mkdir_p("/var/lib/nginx/tmp/fastcgi");
        fs.mkdir_p("/var/lib/nginx/tmp/uwsgi");
        fs.mkdir_p("/var/lib/nginx/tmp/scgi");
        let dev = fs.mkdir_p("/dev");
        let null_id = fs.new_node("null", dev, S_IFCHR | 0o666);
        fs.nodes[null_id].special = Special::Null;
        let zero_id = fs.new_node("zero", dev, S_IFCHR | 0o666);
        fs.nodes[zero_id].special = Special::Zero;
        let urandom_id = fs.new_node("urandom", dev, S_IFCHR | 0o666);
        fs.nodes[urandom_id].special = Special::Urandom;
        let random_id = fs.new_node("random", dev, S_IFCHR | 0o666);
        fs.nodes[random_id].special = Special::Urandom;
    });
    println!("ramfs initialized: {} bytes tar", ROOTFS_TAR.len());
}

fn parse_octal(field: &[u8]) -> usize {
    let mut v = 0usize;
    for &b in field {
        if b >= b'0' && b <= b'7' {
            v = v * 8 + (b - b'0') as usize;
        } else if b == 0 || b == b' ' {
            if v > 0 {
                break;
            }
        }
    }
    v
}

fn unpack_tar(fs: &mut RamFs, data: &[u8]) {
    let mut off = 0usize;
    while off + 512 <= data.len() {
        let header = &data[off..off + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let size = parse_octal(&header[124..136]);
        let typeflag = header[156];
        let mut name = String::from_utf8_lossy(&header[0..100])
            .trim_end_matches('\0')
            .to_string();
        if &header[257..262] == b"ustar" {
            let prefix = String::from_utf8_lossy(&header[345..500])
                .trim_end_matches('\0')
                .to_string();
            if !prefix.is_empty() {
                name = alloc::format!("{}/{}", prefix, name);
            }
        }
        let name = name.trim_start_matches("./").trim_end_matches('/').to_string();
        off += 512;
        if !name.is_empty() {
            match typeflag {
                b'0' | 0 => {
                    // 确保父目录存在
                    if let Some(pos) = name.rfind('/') {
                        fs.mkdir_p(&name[..pos]);
                    }
                    let mode = parse_octal(&header[100..108]) as u32;
                    if let Ok(id) = fs.create_file(&name, "/", mode) {
                        fs.nodes[id].data = data[off..off + size].to_vec();
                    }
                }
                b'5' => {
                    fs.mkdir_p(&name);
                }
                b'2' => {
                    let target = String::from_utf8_lossy(&header[157..257])
                        .trim_end_matches('\0')
                        .to_string();
                    if let Some(pos) = name.rfind('/') {
                        fs.mkdir_p(&name[..pos]);
                    }
                    let _ = fs.create_symlink(&name, "/", &target);
                }
                _ => {}
            }
        }
        off += (size + 511) / 512 * 512;
    }
}
