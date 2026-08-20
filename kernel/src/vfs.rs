//! 文件系统：内嵌 tar rootfs（只读）+ tmpfs 可写覆盖层

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use crate::pmm::spin::Mutex;

// 打包进内核镜像的 rootfs（build 时生成）
static ROOTFS_TAR: &[u8] = include_bytes!("../rootfs.tar");

#[derive(Clone)]
pub struct TarEntry {
    pub name: String,       // 规范化绝对路径（无前导 '/'）
    pub data: &'static [u8],
    pub mode: u32,
    pub kind: TarKind,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TarKind {
    File,
    Dir,
    Symlink,
}

static ROOTFS: Mutex<Vec<TarEntry>> = Mutex::new(Vec::new());

/// tmpfs 可写层：路径 -> 内容
static TMPFS: Mutex<BTreeMap<String, Rc<RefCell<alloc::vec::Vec<u8>>>>> = Mutex::new(BTreeMap::new());
/// tmpfs 已创建目录
static TMPDIRS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn parse_octal(s: &[u8]) -> u64 {
    // GNU tar 可能用 base-256 编码（首位 0x80）
    if !s.is_empty() && s[0] & 0x80 != 0 {
        let mut v: u64 = 0;
        for &b in s {
            v = (v << 8) | (b as u64 & 0xff);
        }
        return v;
    }
    let mut v: u64 = 0;
    for &b in s {
        if b == 0 || b == b' ' {
            break;
        }
        if b.is_ascii_digit() {
            v = v * 8 + (b - b'0') as u64;
        }
    }
    v
}

pub fn init() {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    let mut pending_longname: Option<String> = None;
    while pos + 512 <= ROOTFS_TAR.len() {
        let hdr = &ROOTFS_TAR[pos..pos + 512];
        // 全零块 = 结束
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        let name_bytes = &hdr[0..100];
        let size = parse_octal(&hdr[124..136]) as usize;
        let typeflag = hdr[156];
        let linkname = core::str::from_utf8(&hdr[157..257])
            .unwrap_or("")
            .trim_end_matches('\0')
            .to_string();

        let mut name = match pending_longname.take() {
            Some(long) => long,
            None => String::from(
                core::str::from_utf8(name_bytes)
                    .unwrap_or("")
                    .trim_end_matches('\0'),
            ),
        };
        // ustar prefix 字段
        if &hdr[257..262] == b"ustar" {
            let prefix = core::str::from_utf8(&hdr[345..500])
                .unwrap_or("")
                .trim_end_matches('\0');
            if !prefix.is_empty() {
                name = format!("{}/{}", prefix, name);
            }
        }
        let name = name.trim_matches('/').to_string();

        let kind = match typeflag {
            b'5' => TarKind::Dir,
            b'2' => TarKind::Symlink,
            b'L' => {
                // GNU longname：下一块是真实文件名
                pending_longname = Some(
                    core::str::from_utf8(&ROOTFS_TAR[pos + 512..pos + 512 + size])
                        .unwrap_or("")
                        .trim_end_matches('\0')
                        .to_string(),
                );
                pos += 512 + (size + 511) / 512 * 512;
                continue;
            }
            b'x' | b'g' => {
                // pax 扩展头：跳过
                pos += 512 + (size + 511) / 512 * 512;
                continue;
            }
            _ => TarKind::File,
        };

        let mode = parse_octal(&hdr[100..108]) as u32;
        let data_start = pos + 512;
        let data = &ROOTFS_TAR[data_start..data_start + size];

        entries.push(TarEntry {
            name,
            data,
            mode: mode & 0o7777,
            kind: match kind {
                TarKind::File => TarKind::File,
                TarKind::Dir => TarKind::Dir,
                TarKind::Symlink => TarKind::Symlink,
            },
        });
        // symlink 目标存到哪？复用 linkname —— 存成一个特殊 entry
        if kind == TarKind::Symlink {
            if let Some(e) = entries.last_mut() {
                // 把 target 编码进 name 字段后面? 更简单：单独表
                e.name = format!("{}\0{}", e.name, linkname);
            }
        }

        pos += 512 + (size + 511) / 512 * 512;
    }
    let n = entries.len();
    *ROOTFS.lock() = entries;
    crate::kprintln!("rootfs: {} entries, {} bytes tar", n, ROOTFS_TAR.len());
}

/// 在 rootfs 中查找（返回原始条目）
fn rootfs_find(path: &str) -> Option<TarEntry> {
    let norm = normalize_path(path);
    let rootfs = ROOTFS.lock();
    rootfs
        .iter()
        .find(|e| e.name.split('\0').next().unwrap_or("") == norm)
        .cloned()
}

pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let p = path.trim_start_matches('/');
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(seg),
        }
    }
    parts.join("/")
}

/// 解析路径（含符号链接展开），返回最终目标（路径字符串）
pub fn resolve(path: &str) -> String {
    let mut cur = if path.starts_with('/') {
        String::new()
    } else {
        let cwd = crate::proc::current().cwd.clone();
        normalize_path(&cwd)
    };
    let mut depth = 0;
    let mut comps: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(|s| s.to_string())
        .collect();
    let mut i = 0;
    while i < comps.len() {
        let seg = comps[i].clone();
        if seg.is_empty() || seg == "." {
            comps.remove(i);
            continue;
        }
        if seg == ".." {
            if !cur.is_empty() {
                // 弹出 cur 最后一段
                let mut parts: Vec<&str> = cur.split('/').collect();
                parts.pop();
                cur = parts.join("/");
            }
            comps.remove(i);
            continue;
        }
        // 检查是否符号链接
        let full = if cur.is_empty() {
            seg.clone()
        } else {
            format!("{}/{}", cur, seg)
        };
        if let Some(e) = rootfs_find(&full) {
            if e.kind == TarKind::Symlink {
                depth += 1;
                if depth > 8 {
                    return normalize_path(&full);
                }
                let target = e.name.split('\0').nth(1).unwrap_or("").to_string();
                if target.starts_with('/') {
                    cur = String::new();
                    let mut newc: Vec<String> = target
                        .trim_start_matches('/')
                        .split('/')
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    newc.extend(comps[i + 1..].iter().cloned());
                    comps = newc;
                    i = 0;
                    continue;
                } else {
                    // 相对链接：插入 target 的各段
                    let tsegs: Vec<String> = target
                        .split('/')
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let mut newc = tsegs;
                    newc.extend(comps[i + 1..].iter().cloned());
                    comps = newc;
                    i = 0;
                    continue;
                }
            }
        }
        cur = full;
        i += 1;
    }
    cur
}

/// 文件元信息
pub struct Meta {
    pub exists: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: usize,
    pub mode: u32,
}

pub fn stat_path(path: &str) -> Meta {
    let norm = resolve(path);
    // tmpfs 文件
    if let Some(f) = TMPFS.lock().get(&norm) {
        return Meta {
            exists: true,
            is_dir: false,
            is_symlink: false,
            size: f.borrow().len(),
            mode: 0o644,
        };
    }
    if TMPDIRS.lock().iter().any(|d| *d == norm) {
        return Meta {
            exists: true,
            is_dir: true,
            is_symlink: false,
            size: 0,
            mode: 0o755,
        };
    }
    if norm.is_empty() {
        return Meta {
            exists: true,
            is_dir: true,
            is_symlink: false,
            size: 0,
            mode: 0o755,
        };
    }
    if let Some(e) = rootfs_find(&norm) {
        match e.kind {
            TarKind::Dir => Meta { exists: true, is_dir: true, is_symlink: false, size: 0, mode: if e.mode == 0 { 0o755 } else { e.mode } },
            TarKind::Symlink => Meta { exists: true, is_dir: false, is_symlink: true, size: 0, mode: 0o777 },
            TarKind::File => Meta { exists: true, is_dir: false, is_symlink: false, size: e.data.len(), mode: if e.mode == 0 { 0o644 } else { e.mode } },
        }
    } else {
        Meta { exists: false, is_dir: false, is_symlink: false, size: 0, mode: 0 }
    }
}

/// 打开只读文件：优先 tmpfs，其次 rootfs
pub enum FileData {
    Static(&'static [u8]),
    Tmp(Rc<RefCell<Vec<u8>>>),
}

pub fn open_read(path: &str) -> Option<(FileData, u32)> {
    let norm = resolve(path);
    if let Some(f) = TMPFS.lock().get(&norm) {
        return Some((FileData::Tmp(f.clone()), 0o644));
    }
    let e = rootfs_find(&norm)?;
    if e.kind == TarKind::Dir {
        return None;
    }
    if e.kind == TarKind::Symlink {
        return None;
    }
    Some((FileData::Static(e.data), if e.mode == 0 { 0o644 } else { e.mode }))
}

/// 创建（或打开已有的）tmpfs 可写文件
pub fn create_write(path: &str, truncate: bool) -> Rc<RefCell<Vec<u8>>> {
    let norm = resolve(path);
    let mut tmp = TMPFS.lock();
    if let Some(f) = tmp.get(&norm) {
        if truncate {
            f.borrow_mut().clear();
        }
        return f.clone();
    }
    let f = Rc::new(RefCell::new(Vec::new()));
    tmp.insert(norm, f.clone());
    f
}

/// 创建 tmpfs 目录
pub fn mkdir(path: &str) {
    let norm = resolve(path);
    let mut d = TMPDIRS.lock();
    if !d.iter().any(|x| *x == norm) {
        d.push(norm);
    }
}

/// unlink（只支持 tmpfs）
pub fn unlink(path: &str) -> bool {
    let norm = resolve(path);
    TMPFS.lock().remove(&norm).is_some() || {
        let mut d = TMPDIRS.lock();
        let before = d.len();
        d.retain(|x| *x != norm);
        d.len() != before
    }
}

/// 列目录（合并 rootfs + tmpfs）
pub fn list_dir(path: &str) -> Vec<(String, bool)> {
    let norm = resolve(path);
    let prefix = if norm.is_empty() {
        String::new()
    } else {
        format!("{}/", norm)
    };
    let mut out: Vec<(String, bool)> = Vec::new();
    let rootfs = ROOTFS.lock();
    for e in rootfs.iter() {
        let name = e.name.split('\0').next().unwrap_or("");
        if name.starts_with(&prefix) && name.len() > prefix.len() {
            let rest = &name[prefix.len()..];
            if !rest.contains('/') {
                out.push((rest.to_string(), e.kind == TarKind::Dir));
            } else if let Some(idx) = rest.find('/') {
                let dir = rest[..idx].to_string();
                if !out.iter().any(|(n, _)| *n == dir) {
                    out.push((dir, true));
                }
            }
        }
    }
    for (p, _) in TMPFS.lock().iter() {
        if p.starts_with(&prefix) && p.len() > prefix.len() {
            let rest = &p[prefix.len()..];
            if !rest.contains('/') {
                if !out.iter().any(|(n, _)| n == rest) {
                    out.push((rest.to_string(), false));
                }
            }
        }
    }
    out
}
