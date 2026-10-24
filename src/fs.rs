use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::str;
use spin::Mutex;

pub const ENOENT: i32 = 2;
pub const EIO: i32 = 5;
pub const EBADF: i32 = 9;
pub const EEXIST: i32 = 17;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const EFBIG: i32 = 27;
pub const ELOOP: i32 = 40;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_ACCMODE: u32 = 3;
pub const O_CREAT: u32 = 0x40;
pub const O_EXCL: u32 = 0x80;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
pub const O_DIRECTORY: u32 = 0x10000;
pub const O_NOFOLLOW: u32 = 0x20000;

pub const SEEK_SET: u32 = 0;
pub const SEEK_CUR: u32 = 1;
pub const SEEK_END: u32 = 2;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

const MAX_SYMLINKS: usize = 16;
const HEADER_SIZE: usize = 110;

static FS: Mutex<Option<Fs>> = Mutex::new(None);
static RANDOM_STATE: Mutex<u64> = Mutex::new(0x9e37_79b9_7f4a_7c15);

#[derive(Clone, Copy, Debug)]
pub struct Metadata {
    pub mode: u32,
    pub size: u64,
    pub inode: u64,
}

pub struct File {
    path: String,
    offset: u64,
    flags: u32,
}

struct Fs {
    nodes: BTreeMap<String, Node>,
    next_inode: u64,
}

struct Node {
    mode: u32,
    inode: u64,
    kind: NodeKind,
}

enum NodeKind {
    Directory,
    Regular(FileData),
    Symlink(String),
    Device(Device),
    Other,
}

enum FileData {
    Archive(&'static [u8]),
    Ram(Vec<u8>),
}

#[derive(Clone, Copy)]
enum Device {
    Null,
    Zero,
    Random,
}

pub fn archive() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/rootfs.cpio"))
}

pub fn init() {
    with_fs(|_| {});
}

pub fn read_file(path: &str) -> Result<Vec<u8>, i32> {
    with_fs(|fs| {
        let path = resolve_path(fs, path, true)?;
        let node = fs.nodes.get(&path).ok_or(ENOENT)?;
        match &node.kind {
            NodeKind::Regular(data) => Ok(data.as_slice().to_vec()),
            NodeKind::Directory => Err(EISDIR),
            NodeKind::Device(_) | NodeKind::Other => Err(ENODEV),
            NodeKind::Symlink(_) => Err(ELOOP),
        }
    })
}

pub fn metadata(path: &str, follow: bool) -> Result<Metadata, i32> {
    with_fs(|fs| {
        let path = resolve_path(fs, path, follow)?;
        fs.nodes.get(&path).map(Node::metadata).ok_or(ENOENT)
    })
}

pub fn read_dir(path: &str) -> Result<Vec<(String, Metadata)>, i32> {
    with_fs(|fs| {
        let directory = resolve_path(fs, path, true)?;
        let node = fs.nodes.get(&directory).ok_or(ENOENT)?;
        if !matches!(node.kind, NodeKind::Directory) {
            return Err(ENOTDIR);
        }

        let mut entries = Vec::new();
        for (path, node) in &fs.nodes {
            if path != &directory && parent_path(path) == directory {
                if let Some(name) = file_name(path) {
                    entries.push((name.to_string(), node.metadata()));
                }
            }
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(entries)
    })
}

pub fn mkdir(path: &str, mode: u32) -> Result<(), i32> {
    with_fs(|fs| {
        let path = resolve_path(fs, path, false)?;
        if fs.nodes.contains_key(&path) {
            return Err(EEXIST);
        }
        require_parent_directory(fs, &path)?;
        let inode = fs.allocate_inode();
        fs.nodes.insert(
            path,
            Node {
                mode: S_IFDIR | (mode & 0o7777),
                inode,
                kind: NodeKind::Directory,
            },
        );
        Ok(())
    })
}

pub fn unlink(path: &str) -> Result<(), i32> {
    with_fs(|fs| {
        let path = resolve_path(fs, path, false)?;
        let node = fs.nodes.get(&path).ok_or(ENOENT)?;
        if matches!(node.kind, NodeKind::Directory) {
            return Err(EISDIR);
        }
        fs.nodes.remove(&path);
        Ok(())
    })
}

pub fn open(path: &str, flags: u32, mode: u32) -> Result<File, i32> {
    with_fs(|fs| {
        let original = normalize_path(path)?;
        let without_final_follow = resolve_path(fs, &original, false)?;
        let unfollowed = fs.nodes.get(&without_final_follow);

        if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) && unfollowed.is_some() {
            return Err(EEXIST);
        }
        if flags & O_NOFOLLOW != 0
            && unfollowed.is_some_and(|node| matches!(node.kind, NodeKind::Symlink(_)))
        {
            return Err(ELOOP);
        }

        let path = resolve_path(fs, &original, true)?;
        if let Some(node) = fs.nodes.get(&path) {
            if flags & O_DIRECTORY != 0 && !matches!(node.kind, NodeKind::Directory) {
                return Err(ENOTDIR);
            }
            match node.kind {
                NodeKind::Directory if flags & O_ACCMODE != O_RDONLY => return Err(EISDIR),
                NodeKind::Other => return Err(ENODEV),
                NodeKind::Symlink(_) => return Err(ELOOP),
                _ => {}
            }

            if flags & O_TRUNC != 0 {
                let node = fs.nodes.get_mut(&path).expect("node disappeared");
                match &mut node.kind {
                    NodeKind::Regular(data) => data.ram_mut().clear(),
                    NodeKind::Directory => return Err(EISDIR),
                    NodeKind::Device(_) => {}
                    NodeKind::Other => return Err(ENODEV),
                    NodeKind::Symlink(_) => return Err(ELOOP),
                }
            }
        } else {
            if flags & O_CREAT == 0 {
                return Err(ENOENT);
            }
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            require_parent_directory(fs, &path)?;
            let inode = fs.allocate_inode();
            fs.nodes.insert(
                path.clone(),
                Node {
                    mode: S_IFREG | (mode & 0o7777),
                    inode,
                    kind: NodeKind::Regular(FileData::Ram(Vec::new())),
                },
            );
        }

        Ok(File {
            path,
            offset: 0,
            flags,
        })
    })
}

impl File {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        if self.flags & O_ACCMODE == O_WRONLY {
            return Err(EBADF);
        }
        with_fs(|fs| {
            let node = fs.nodes.get(&self.path).ok_or(ENOENT)?;
            match &node.kind {
                NodeKind::Regular(data) => {
                    let data = data.as_slice();
                    let start = match usize::try_from(self.offset) {
                        Ok(start) if start < data.len() => start,
                        _ => return Ok(0),
                    };
                    let count = buf.len().min(data.len() - start);
                    buf[..count].copy_from_slice(&data[start..start + count]);
                    self.offset += count as u64;
                    Ok(count)
                }
                NodeKind::Directory => Err(EISDIR),
                NodeKind::Device(Device::Null) => Ok(0),
                NodeKind::Device(Device::Zero) => {
                    let next = self
                        .offset
                        .checked_add(buf.len() as u64)
                        .ok_or(EFBIG)?;
                    buf.fill(0);
                    self.offset = next;
                    Ok(buf.len())
                }
                NodeKind::Device(Device::Random) => {
                    let next = self
                        .offset
                        .checked_add(buf.len() as u64)
                        .ok_or(EFBIG)?;
                    fill_random(buf);
                    self.offset = next;
                    Ok(buf.len())
                }
                NodeKind::Other => Err(ENODEV),
                NodeKind::Symlink(_) => Err(ELOOP),
            }
        })
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        if self.flags & O_ACCMODE == O_RDONLY {
            return Err(EBADF);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        with_fs(|fs| {
            let node = fs.nodes.get_mut(&self.path).ok_or(ENOENT)?;
            match &mut node.kind {
                NodeKind::Regular(data) => {
                    let data = data.ram_mut();
                    let start = if self.flags & O_APPEND != 0 {
                        data.len()
                    } else {
                        usize::try_from(self.offset).map_err(|_| EFBIG)?
                    };
                    let end = start.checked_add(buf.len()).ok_or(EFBIG)?;
                    if start > data.len() {
                        data.resize(start, 0);
                    }
                    if end > data.len() {
                        data.resize(end, 0);
                    }
                    data[start..end].copy_from_slice(buf);
                    self.offset = end as u64;
                    Ok(buf.len())
                }
                NodeKind::Directory => Err(EISDIR),
                NodeKind::Device(_) => Ok(buf.len()),
                NodeKind::Other => Err(ENODEV),
                NodeKind::Symlink(_) => Err(ELOOP),
            }
        })
    }

    pub fn seek(&mut self, offset: i64, whence: u32) -> Result<u64, i32> {
        let next = match whence {
            SEEK_SET => offset as i128,
            SEEK_CUR => self.offset as i128 + offset as i128,
            SEEK_END => self.metadata()?.size as i128 + offset as i128,
            _ => return Err(EINVAL),
        };
        if next < 0 || next > u64::MAX as i128 {
            return Err(EINVAL);
        }
        self.offset = next as u64;
        Ok(self.offset)
    }

    pub fn metadata(&self) -> Result<Metadata, i32> {
        metadata(&self.path, true)
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Node {
    fn metadata(&self) -> Metadata {
        let size = match &self.kind {
            NodeKind::Regular(data) => data.as_slice().len() as u64,
            NodeKind::Symlink(target) => target.len() as u64,
            _ => 0,
        };
        Metadata {
            mode: self.mode,
            size,
            inode: self.inode,
        }
    }
}

impl FileData {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Archive(data) => data,
            Self::Ram(data) => data,
        }
    }

    fn ram_mut(&mut self) -> &mut Vec<u8> {
        if let Self::Archive(data) = self {
            let data = data.to_vec();
            *self = Self::Ram(data);
        }
        match self {
            Self::Ram(data) => data,
            Self::Archive(_) => unreachable!(),
        }
    }
}

impl Fs {
    fn parse(archive: &'static [u8]) -> Result<Self, i32> {
        let mut fs = Self {
            nodes: BTreeMap::new(),
            next_inode: 1,
        };
        let root_inode = fs.allocate_inode();
        fs.nodes.insert(
            "/".to_string(),
            Node {
                mode: S_IFDIR | 0o755,
                inode: root_inode,
                kind: NodeKind::Directory,
            },
        );

        let mut offset = 0usize;
        loop {
            let header_end = offset.checked_add(HEADER_SIZE).ok_or(EIO)?;
            if header_end > archive.len() {
                return Err(EIO);
            }
            if &archive[offset..offset + 6] != b"070701" {
                return Err(EINVAL);
            }

            let mut fields = [0u32; 13];
            for (index, field) in fields.iter_mut().enumerate() {
                let start = offset + 6 + index * 8;
                *field = parse_hex(&archive[start..start + 8])?;
            }

            let name_size = fields[11] as usize;
            if name_size == 0 {
                return Err(EINVAL);
            }
            let name_end = header_end.checked_add(name_size).ok_or(EIO)?;
            if name_end > archive.len() || archive[name_end - 1] != 0 {
                return Err(EIO);
            }
            let raw_name = &archive[header_end..name_end - 1];
            if raw_name.contains(&0) {
                return Err(EINVAL);
            }

            let data_start = align_four(name_end)?;
            let data_end = data_start
                .checked_add(fields[6] as usize)
                .ok_or(EIO)?;
            let next = align_four(data_end)?;
            if next > archive.len() {
                return Err(EIO);
            }

            if raw_name == b"TRAILER!!!" {
                fs.install_devices()?;
                return Ok(fs);
            }

            let name = str::from_utf8(raw_name).map_err(|_| EINVAL)?;
            let path = normalize_path(name)?;
            let mode = fields[1];
            let data = &archive[data_start..data_end];

            if path == "/" {
                if mode & S_IFMT != S_IFDIR {
                    return Err(ENOTDIR);
                }
                if let Some(root) = fs.nodes.get_mut("/") {
                    root.mode = mode;
                }
                offset = next;
                continue;
            }

            fs.ensure_parent_directories(&path)?;
            let inode = fs.allocate_inode();
            let kind = match mode & S_IFMT {
                S_IFREG => NodeKind::Regular(FileData::Archive(data)),
                S_IFDIR => NodeKind::Directory,
                S_IFLNK => {
                    let target = str::from_utf8(data).map_err(|_| EINVAL)?;
                    if target.is_empty() || target.as_bytes().contains(&0) {
                        return Err(EINVAL);
                    }
                    NodeKind::Symlink(target.to_string())
                }
                _ => NodeKind::Other,
            };
            fs.nodes.insert(
                path,
                Node {
                    mode,
                    inode,
                    kind,
                },
            );
            offset = next;
        }
    }

    fn allocate_inode(&mut self) -> u64 {
        let inode = self.next_inode;
        self.next_inode = self.next_inode.checked_add(1).expect("inode exhausted");
        inode
    }

    fn ensure_parent_directories(&mut self, path: &str) -> Result<(), i32> {
        let parent = parent_path(path);
        if parent == "/" {
            return Ok(());
        }

        let mut current = String::new();
        for component in parent.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(component);
            if let Some(node) = self.nodes.get(&current) {
                if !matches!(node.kind, NodeKind::Directory) {
                    return Err(ENOTDIR);
                }
            } else {
                let inode = self.allocate_inode();
                self.nodes.insert(
                    current.clone(),
                    Node {
                        mode: S_IFDIR | 0o755,
                        inode,
                        kind: NodeKind::Directory,
                    },
                );
            }
        }
        Ok(())
    }

    fn install_devices(&mut self) -> Result<(), i32> {
        for (path, device) in [
            ("/dev/null", Device::Null),
            ("/dev/zero", Device::Zero),
            ("/dev/urandom", Device::Random),
        ] {
            self.ensure_parent_directories(path)?;
            let inode = self.allocate_inode();
            self.nodes.insert(
                path.to_string(),
                Node {
                    mode: S_IFCHR | 0o666,
                    inode,
                    kind: NodeKind::Device(device),
                },
            );
        }
        Ok(())
    }
}

fn with_fs<R>(function: impl FnOnce(&mut Fs) -> R) -> R {
    let mut guard = FS.lock();
    if guard.is_none() {
        let fs = Fs::parse(archive())
            .unwrap_or_else(|error| panic!("invalid rootfs CPIO archive: {error}"));
        *guard = Some(fs);
    }
    function(guard.as_mut().expect("filesystem initialized"))
}

fn normalize_path(path: &str) -> Result<String, i32> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    if path.as_bytes().contains(&0) {
        return Err(EINVAL);
    }

    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }

    let mut normalized = String::from("/");
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    Ok(normalized)
}

fn resolve_path(fs: &Fs, path: &str, follow_final: bool) -> Result<String, i32> {
    let mut current = normalize_path(path)?;
    let mut followed = 0usize;

    'resolve: loop {
        let components: Vec<&str> = current.split('/').filter(|part| !part.is_empty()).collect();
        let mut prefix = String::from("/");

        for (index, component) in components.iter().enumerate() {
            if prefix != "/" {
                prefix.push('/');
            }
            prefix.push_str(component);
            let last = index + 1 == components.len();
            let node = fs.nodes.get(&prefix);

            if !last {
                match node {
                    None => return Err(ENOENT),
                    Some(node)
                        if !matches!(node.kind, NodeKind::Directory | NodeKind::Symlink(_)) =>
                    {
                        return Err(ENOTDIR);
                    }
                    _ => {}
                }
            }

            let target = match node {
                Some(Node {
                    kind: NodeKind::Symlink(target),
                    ..
                }) if !last || follow_final => target,
                _ => continue,
            };

            followed += 1;
            if followed > MAX_SYMLINKS {
                return Err(ELOOP);
            }

            let mut next = if target.starts_with('/') {
                normalize_path(target)?
            } else {
                let parent = parent_path(&prefix);
                let mut combined = String::from(parent);
                if combined != "/" {
                    combined.push('/');
                }
                combined.push_str(target);
                normalize_path(&combined)?
            };
            for remaining in &components[index + 1..] {
                if next != "/" {
                    next.push('/');
                }
                next.push_str(remaining);
            }
            current = normalize_path(&next)?;
            continue 'resolve;
        }

        return Ok(current);
    }
}

fn require_parent_directory(fs: &Fs, path: &str) -> Result<(), i32> {
    if path == "/" {
        return Err(EEXIST);
    }
    let parent = parent_path(path);
    let node = fs.nodes.get(parent).ok_or(ENOENT)?;
    if matches!(node.kind, NodeKind::Directory) {
        Ok(())
    } else {
        Err(ENOTDIR)
    }
}

fn parent_path(path: &str) -> &str {
    if path == "/" {
        return "/";
    }
    match path.rfind('/') {
        Some(0) => "/",
        Some(index) => &path[..index],
        None => "/",
    }
}

fn file_name(path: &str) -> Option<&str> {
    if path == "/" {
        None
    } else {
        path.rsplit('/').next().filter(|name| !name.is_empty())
    }
}

fn align_four(value: usize) -> Result<usize, i32> {
    value.checked_add(3).map(|value| value & !3).ok_or(EIO)
}

fn parse_hex(bytes: &[u8]) -> Result<u32, i32> {
    let mut value = 0u32;
    for byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as u32,
            b'a'..=b'f' => (byte - b'a' + 10) as u32,
            b'A'..=b'F' => (byte - b'A' + 10) as u32,
            _ => return Err(EINVAL),
        };
        value = value.checked_mul(16).and_then(|value| value.checked_add(digit)).ok_or(EINVAL)?;
    }
    Ok(value)
}

fn fill_random(buf: &mut [u8]) {
    let mut state = RANDOM_STATE.lock();
    let mut word = 0u64;
    for (index, byte) in buf.iter_mut().enumerate() {
        if index % 8 == 0 {
            let mut next = *state;
            next ^= next << 13;
            next ^= next >> 7;
            next ^= next << 17;
            *state = next;
            word = next;
        }
        *byte = word as u8;
        word >>= 8;
    }
}
