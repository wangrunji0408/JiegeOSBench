//! In-memory filesystem populated from an embedded ustar archive, plus the
//! fd-table object model.
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub static ROOTFS_TAR: &[u8] = include_bytes!("../build/rootfs.tar");

pub type FileData = Arc<Mutex<Vec<u8>>>;

pub struct RamFs {
    pub files: BTreeMap<String, FileData>,
    pub dirs: BTreeSet<String>,
}

static FS: Mutex<Option<RamFs>> = Mutex::new(None);

pub fn with_fs<R>(f: impl FnOnce(&mut RamFs) -> R) -> R {
    let mut g = FS.lock();
    f(g.as_mut().expect("fs not initialized"))
}

/// Normalize a path: absolute, no trailing slash (except "/"), resolves "." and "..".
pub fn normalize(cwd: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let full = if path.starts_with('/') {
        path.to_string()
    } else {
        alloc::format!("{}/{}", cwd, path)
    };
    for seg in full.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        let mut s = String::new();
        for p in parts {
            s.push('/');
            s.push_str(p);
        }
        s
    }
}

fn parse_octal(b: &[u8]) -> usize {
    let mut v = 0;
    for &c in b {
        if c.is_ascii_digit() {
            v = v * 8 + (c - b'0') as usize;
        }
    }
    v
}

pub fn init() {
    let mut fs = RamFs {
        files: BTreeMap::new(),
        dirs: BTreeSet::new(),
    };
    fs.dirs.insert("/".to_string());

    let tar = ROOTFS_TAR;
    let mut off = 0;
    let mut nfiles = 0;
    while off + 512 <= tar.len() {
        let hdr = &tar[off..off + 512];
        if hdr[0] == 0 {
            break;
        }
        let name_end = hdr[..100].iter().position(|&c| c == 0).unwrap_or(100);
        let mut name = core::str::from_utf8(&hdr[..name_end]).unwrap().to_string();
        // ustar prefix field
        let prefix_end = hdr[345..500].iter().position(|&c| c == 0).unwrap_or(155);
        if prefix_end > 0 {
            let prefix = core::str::from_utf8(&hdr[345..345 + prefix_end]).unwrap();
            name = alloc::format!("{}/{}", prefix, name);
        }
        let size = parse_octal(&hdr[124..136]);
        let typeflag = hdr[156];
        let path = normalize("/", name.trim_start_matches('.'));
        match typeflag {
            b'0' | 0 => {
                let data = tar[off + 512..off + 512 + size].to_vec();
                fs.files.insert(path.clone(), Arc::new(Mutex::new(data)));
                nfiles += 1;
                // ensure parent dirs exist
                let mut p = path.as_str();
                while let Some(idx) = p.rfind('/') {
                    let d = if idx == 0 { "/" } else { &p[..idx] };
                    fs.dirs.insert(d.to_string());
                    p = d;
                    if d == "/" {
                        break;
                    }
                }
            }
            b'5' => {
                fs.dirs.insert(path.trim_end_matches('/').to_string());
            }
            _ => {}
        }
        off += 512 + (size + 511) / 512 * 512;
    }
    println!(
        "[fs] ramfs: {} files, {} dirs from {} KiB tar",
        nfiles,
        fs.dirs.len(),
        tar.len() / 1024
    );
    *FS.lock() = Some(fs);
}

impl RamFs {
    pub fn lookup_file(&self, path: &str) -> Option<FileData> {
        self.files.get(path).cloned()
    }
    pub fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains(path)
    }
    pub fn create(&mut self, path: &str) -> FileData {
        let f: FileData = Arc::new(Mutex::new(Vec::new()));
        self.files.insert(path.to_string(), f.clone());
        f
    }
    pub fn unlink(&mut self, path: &str) -> bool {
        self.files.remove(path).is_some()
    }
}

/// What an fd points to.
#[derive(Clone)]
pub enum FdObj {
    File {
        data: FileData,
        pos: usize,
        append: bool,
        path: String,
    },
    Dir {
        path: String,
    },
    Stdio, // console (stdin/stdout/stderr)
    Null,
    Socket(usize),
    Epoll(usize),
    EventFd {
        val: Arc<Mutex<u64>>,
        semaphore: bool,
    },
}

#[derive(Clone)]
pub struct FdEntry {
    pub obj: FdObj,
    pub cloexec: bool,
    pub nonblock: bool,
}

pub struct FdTable {
    pub entries: Vec<Option<FdEntry>>,
}

impl FdTable {
    pub fn new() -> Self {
        let stdio = FdEntry {
            obj: FdObj::Stdio,
            cloexec: false,
            nonblock: false,
        };
        FdTable {
            entries: alloc::vec![Some(stdio.clone()), Some(stdio.clone()), Some(stdio)],
        }
    }
    pub fn get(&self, fd: usize) -> Option<&FdEntry> {
        self.entries.get(fd).and_then(|e| e.as_ref())
    }
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FdEntry> {
        self.entries.get_mut(fd).and_then(|e| e.as_mut())
    }
    pub fn alloc(&mut self, e: FdEntry) -> usize {
        for (i, slot) in self.entries.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(e);
                return i;
            }
        }
        self.entries.push(Some(e));
        self.entries.len() - 1
    }
    pub fn set(&mut self, fd: usize, e: FdEntry) {
        if fd >= self.entries.len() {
            self.entries.resize(fd + 1, None);
        }
        self.entries[fd] = Some(e);
    }
    pub fn close(&mut self, fd: usize) -> Option<FdEntry> {
        self.entries.get_mut(fd).and_then(|e| e.take())
    }
}
