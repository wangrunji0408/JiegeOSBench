//! Populate the ramfs from a cpio "newc" archive in memory.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::vfs::{lookup_parent, root, Dentry, NodeKind};
use crate::abi::*;

fn hex(b: &[u8]) -> u64 {
    let mut v = 0u64;
    for &c in b {
        v = v * 16
            + match c {
                b'0'..=b'9' => (c - b'0') as u64,
                b'a'..=b'f' => (c - b'a' + 10) as u64,
                b'A'..=b'F' => (c - b'A' + 10) as u64,
                _ => 0,
            };
    }
    v
}

/// Parse the archive at `base`; returns the end address (after the trailer).
pub fn load(base: usize) -> Result<usize, &'static str> {
    let mut off = 0usize;
    let mut count = 0;
    loop {
        let hdr = unsafe { core::slice::from_raw_parts((base + off) as *const u8, 110) };
        if &hdr[0..6] != b"070701" {
            if count == 0 {
                return Err("bad cpio magic");
            }
            return Err("bad cpio header");
        }
        let mode = hex(&hdr[14..22]) as u32;
        let uid = hex(&hdr[22..30]) as u32;
        let gid = hex(&hdr[30..38]) as u32;
        let mtime = hex(&hdr[46..54]) as i64;
        let filesize = hex(&hdr[54..62]) as usize;
        let rdev_major = hex(&hdr[78..86]) as u32;
        let rdev_minor = hex(&hdr[86..94]) as u32;
        let namesize = hex(&hdr[94..102]) as usize;
        let name_start = off + 110;
        let name = unsafe { core::slice::from_raw_parts((base + name_start) as *const u8, namesize.saturating_sub(1)) };
        let name = String::from_utf8_lossy(name).into_owned();
        let data_start = (name_start + namesize + 3) & !3;
        let data = unsafe { core::slice::from_raw_parts((base + data_start) as *const u8, filesize) };
        off = (data_start + filesize + 3) & !3;
        count += 1;
        if name == "TRAILER!!!" {
            return Ok(base + off);
        }
        let path = name.trim_start_matches("./").trim_start_matches('/');
        if path.is_empty() || path == "." {
            continue;
        }
        let kind = match mode & S_IFMT {
            S_IFDIR => NodeKind::Dir(crate::sync::SpinLock::new(alloc::collections::BTreeMap::new())),
            S_IFREG => NodeKind::File(crate::sync::SpinLock::new(data.to_vec())),
            S_IFLNK => NodeKind::Symlink(String::from_utf8_lossy(data).into_owned()),
            S_IFCHR => NodeKind::CharDev(rdev_major, rdev_minor),
            S_IFIFO => NodeKind::Fifo,
            S_IFSOCK => NodeKind::Socket,
            _ => continue,
        };
        let (parent, fname) = match lookup_parent(&root(), path) {
            Ok(p) => p,
            Err(_) => {
                // create missing parent directories
                let dir = &path[..path.rfind('/').unwrap_or(0)];
                super::vfs::mkdir_p(dir);
                lookup_parent(&root(), path).map_err(|_| "cpio: bad parent")?
            }
        };
        if let Some(existing) = parent.lookup_child(&fname) {
            // directory already created implicitly: just update metadata
            if existing.is_dir() && matches!(kind, NodeKind::Dir(_)) {
                let mut m = existing.meta.lock();
                m.mode = mode;
                m.uid = uid;
                m.gid = gid;
                m.mtime = mtime;
                continue;
            }
            let _ = parent.remove_child(&fname);
        }
        let d = Dentry::new(&fname, Arc::downgrade(&parent), mode, kind);
        {
            let mut m = d.meta.lock();
            m.uid = uid;
            m.gid = gid;
            m.mtime = mtime;
            m.ctime = mtime;
            m.atime = mtime;
            if mode & S_IFMT == S_IFCHR {
                m.rdev = ((rdev_major as u64) << 8) | rdev_minor as u64;
            }
        }
        parent.add_child(d).map_err(|_| "cpio: add child")?;
    }
}

pub fn count_files(d: &Arc<Dentry>) -> usize {
    let mut n = 1;
    for (_, c) in d.children() {
        n += count_files(&c);
    }
    n
}

pub fn _unused() -> Vec<u8> {
    Vec::new()
}
