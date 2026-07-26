//! Path resolution.

use super::inode::{Inode, InodeKind, InodeRef};
use super::{Error, Result};
use crate::bail;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MAX_SYMLINKS: usize = 40;

/// Split a path into components, dropping empty ones and `.`.
fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

/// Resolve `path` relative to `cwd` (or the root if the path is absolute).
///
/// `follow_final` controls whether a trailing symlink is dereferenced —
/// `lstat` and `readlink` need it off.
pub fn resolve_from(cwd: &InodeRef, path: &str, follow_final: bool) -> Result<InodeRef> {
    resolve_inner(cwd, path, follow_final, 0)
}

fn resolve_inner(cwd: &InodeRef, path: &str, follow_final: bool, depth: usize) -> Result<InodeRef> {
    if depth > MAX_SYMLINKS {
        bail!(ELOOP);
    }
    if path.len() > 4096 {
        bail!(ENAMETOOLONG);
    }

    let mut current: InodeRef = if path.starts_with('/') {
        super::root().clone()
    } else {
        cwd.clone()
    };

    let parts = components(path);
    if parts.is_empty() {
        // Either "/" or "." — resolve to the starting directory.
        return Ok(current);
    }

    for (i, part) in parts.iter().enumerate() {
        let is_final = i == parts.len() - 1;
        if current.kind() != InodeKind::Dir {
            bail!(ENOTDIR);
        }
        let next = current.lookup(part)?;

        // Follow symlinks on intermediate components always, and on the final
        // one only if asked.
        if next.kind() == InodeKind::Symlink && (!is_final || follow_final) {
            let target = next.readlink()?;
            let base = if target.starts_with('/') {
                super::root().clone()
            } else {
                current.clone()
            };
            current = resolve_inner(&base, &target, true, depth + 1)?;
        } else {
            current = next;
        }
    }
    Ok(current)
}

/// Resolve relative to the current task's cwd.
pub fn resolve(path: &str, follow_final: bool) -> Result<InodeRef> {
    let cwd = crate::task::current_cwd();
    resolve_from(&cwd, path, follow_final)
}

/// Split a path into (parent directory path, final component).
pub fn split_parent(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => ("/", &trimmed[1..]),
        Some(idx) => (&trimmed[..idx], &trimmed[idx + 1..]),
        None => (".", trimmed),
    }
}

/// Resolve the parent directory of `path` and return it with the final name.
pub fn resolve_parent_from(cwd: &InodeRef, path: &str) -> Result<(InodeRef, String)> {
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        bail!(EINVAL);
    }
    let dir = resolve_from(cwd, parent, true)?;
    if dir.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    Ok((dir, name.to_string()))
}

pub fn resolve_parent(path: &str) -> Result<(InodeRef, String)> {
    let cwd = crate::task::current_cwd();
    resolve_parent_from(&cwd, path)
}

/// `mkdir -p`: create a directory and any missing parents.
pub fn mkdir_p(path: &str, mode: u32) -> Result<InodeRef> {
    let mut current = super::root().clone();
    for part in components(path) {
        current = match current.lookup(part) {
            Ok(inode) => {
                if inode.kind() == InodeKind::Symlink {
                    let target = inode.readlink()?;
                    resolve_from(&current, &target, true)?
                } else {
                    inode
                }
            }
            Err(_) => current.create(part, InodeKind::Dir, mode)?,
        };
        if current.kind() != InodeKind::Dir {
            bail!(ENOTDIR);
        }
    }
    Ok(current)
}

/// Create a file, creating parent directories as needed. Used to seed the rootfs.
pub fn create_file(path: &str, mode: u32, data: Vec<u8>) -> Result<InodeRef> {
    let (parent_path, name) = split_parent(path);
    let dir = mkdir_p(parent_path, 0o755)?;
    // Replace any existing entry.
    let _ = dir.unlink(&name);
    let file = super::ramfs::RamFile::with_data(mode, data);
    dir.link(&name, &(file.clone() as InodeRef))?;
    Ok(file)
}

/// Build the absolute path of an inode by walking up from it. Used by `getcwd`
/// and `/proc/self/exe`. Returns `None` if the inode isn't reachable from root
/// (which shouldn't happen for directories).
pub fn abs_path(target: &InodeRef) -> Option<String> {
    if target.ino() == super::root().ino() {
        return Some("/".to_string());
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = target.clone();
    for _ in 0..MAX_SYMLINKS {
        let parent = current.lookup("..").ok()?;
        if parent.ino() == current.ino() {
            break; // reached root
        }
        // Find our name in the parent.
        let name = parent
            .readdir()
            .ok()?
            .into_iter()
            .find(|e| e.ino == current.ino() && e.name != "." && e.name != "..")
            .map(|e| e.name)?;
        parts.push(name);
        if parent.ino() == super::root().ino() {
            break;
        }
        current = parent;
    }
    parts.reverse();
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    Some(out)
}

/// Normalize a path lexically: collapse `//`, `.` and `..`.
pub fn normalize(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            p => stack.push(p),
        }
    }
    let joined = stack.join("/");
    if path.starts_with('/') {
        alloc::format!("/{}", joined)
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// Interpret a `dirfd` + path pair the way the `*at` syscalls do.
pub const AT_FDCWD: i32 = -100;
pub const AT_SYMLINK_NOFOLLOW: usize = 0x100;
pub const AT_REMOVEDIR: usize = 0x200;
pub const AT_SYMLINK_FOLLOW: usize = 0x400;
pub const AT_EMPTY_PATH: usize = 0x1000;

/// Resolve the starting directory for a `*at` syscall.
pub fn at_base(dirfd: i32) -> Result<InodeRef> {
    if dirfd == AT_FDCWD {
        return Ok(crate::task::current_cwd());
    }
    let file = crate::task::current()
        .files
        .lock()
        .get(dirfd)
        .ok_or(Error::new(super::errno::EBADF))?;
    Ok(file.inode.clone())
}

/// Resolve a `*at`-style (dirfd, path) pair.
pub fn resolve_at(dirfd: i32, path: &str, follow: bool) -> Result<InodeRef> {
    if path.starts_with('/') {
        return resolve_from(super::root(), path, follow);
    }
    let base = at_base(dirfd)?;
    if path.is_empty() {
        return Ok(base);
    }
    resolve_from(&base, path, follow)
}

/// Resolve the parent for a `*at`-style pair.
pub fn resolve_parent_at(dirfd: i32, path: &str) -> Result<(InodeRef, String)> {
    if path.starts_with('/') {
        return resolve_parent_from(super::root(), path);
    }
    let base = at_base(dirfd)?;
    resolve_parent_from(&base, path)
}
