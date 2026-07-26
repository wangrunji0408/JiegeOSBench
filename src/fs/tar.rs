//! Unpack a POSIX tar archive into the ramfs.
//!
//! The rootfs (nginx, musl, its shared libraries and config) is built on the
//! host into a tar file, embedded in the kernel image with `include_bytes!`, and
//! extracted here at boot.

use super::inode::{InodeKind, InodeRef};
use super::{path, Result};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const BLOCK: usize = 512;

struct Header<'a>(&'a [u8]);

impl<'a> Header<'a> {
    fn field(&self, off: usize, len: usize) -> &'a [u8] {
        &self.0[off..off + len]
    }

    fn str_field(&self, off: usize, len: usize) -> String {
        let raw = self.field(off, len);
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).into_owned()
    }

    /// Parse an octal numeric field, tolerating leading spaces and both
    /// space- and NUL-terminated forms.
    fn octal(&self, off: usize, len: usize) -> u64 {
        let raw = self.field(off, len);
        let mut value = 0u64;
        for &b in raw {
            match b {
                b'0'..=b'7' => value = value * 8 + (b - b'0') as u64,
                b' ' | 0 => {
                    if value != 0 {
                        break;
                    }
                }
                _ => break,
            }
        }
        value
    }

    fn name(&self) -> String {
        let prefix = self.str_field(345, 155);
        let name = self.str_field(0, 100);
        if prefix.is_empty() {
            name
        } else {
            alloc::format!("{}/{}", prefix, name)
        }
    }

    fn mode(&self) -> u32 {
        self.octal(100, 8) as u32
    }

    fn uid(&self) -> u32 {
        self.octal(108, 8) as u32
    }

    fn gid(&self) -> u32 {
        self.octal(116, 8) as u32
    }

    fn size(&self) -> usize {
        self.octal(124, 12) as usize
    }

    fn typeflag(&self) -> u8 {
        self.0[156]
    }

    fn linkname(&self) -> String {
        self.str_field(157, 100)
    }

    fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }
}

/// Extract a tar archive, returning the number of entries created.
pub fn extract(data: &[u8]) -> Result<usize> {
    let mut offset = 0;
    let mut count = 0;
    // GNU long name/link extensions carry the real name in a preceding entry.
    let mut pending_longname: Option<String> = None;
    let mut pending_longlink: Option<String> = None;

    while offset + BLOCK <= data.len() {
        let header = Header(&data[offset..offset + BLOCK]);
        if header.is_zero() {
            // Two zero blocks mark the end, but a single one is enough for us.
            break;
        }
        let size = header.size();
        let typeflag = header.typeflag();
        let name = pending_longname
            .take()
            .unwrap_or_else(|| header.name());
        let body_start = offset + BLOCK;
        let body_end = (body_start + size).min(data.len());
        let body = &data[body_start..body_end];
        // Advance past the header and the data, rounded up to a block.
        offset = body_start + (size + BLOCK - 1) / BLOCK * BLOCK;

        match typeflag {
            b'L' => {
                // GNU long name for the next entry.
                let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                pending_longname = Some(String::from_utf8_lossy(&body[..end]).into_owned());
                continue;
            }
            b'K' => {
                let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                pending_longlink = Some(String::from_utf8_lossy(&body[..end]).into_owned());
                continue;
            }
            // Pax extended headers: skip; we don't need their metadata.
            b'x' | b'g' => continue,
            _ => {}
        }

        let abs = normalize_entry(&name);
        if abs.is_empty() || abs == "/" {
            continue;
        }

        match typeflag {
            b'0' | 0 | b'7' => {
                // Regular file.
                let inode = path::create_file(&abs, header.mode(), body.to_vec())?;
                inode.set_owner(header.uid(), header.gid());
                count += 1;
            }
            b'5' => {
                // Directory.
                let inode = path::mkdir_p(&abs, header.mode())?;
                inode.set_owner(header.uid(), header.gid());
                count += 1;
            }
            b'2' => {
                // Symlink.
                let target = pending_longlink.take().unwrap_or_else(|| header.linkname());
                let (parent_path, base) = path::split_parent(&abs);
                let dir = path::mkdir_p(parent_path, 0o755)?;
                let _ = dir.unlink(&base);
                let _ = dir.symlink(&base, &target);
                count += 1;
            }
            b'1' => {
                // Hard link.
                let target = pending_longlink.take().unwrap_or_else(|| header.linkname());
                let target_abs = normalize_entry(&target);
                if let Ok(inode) = path::resolve_from(super::root(), &target_abs, false) {
                    let (parent_path, base) = path::split_parent(&abs);
                    let dir = path::mkdir_p(parent_path, 0o755)?;
                    let _ = dir.unlink(&base);
                    let _ = dir.link(&base, &inode);
                    count += 1;
                }
            }
            b'3' | b'4' | b'6' => {
                // Device / FIFO entries in the archive: we create the ones we
                // need in `device::init` instead.
            }
            other => {
                crate::warn!("tar: skipping entry {} of unknown type {}", abs, other as char);
            }
        }
    }
    Ok(count)
}

/// Turn a tar entry name into an absolute path.
fn normalize_entry(name: &str) -> String {
    let trimmed = name.trim_start_matches("./").trim_start_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let joined = alloc::format!("/{}", trimmed);
    let normalized = path::normalize(&joined);
    normalized.trim_end_matches('/').to_string()
}

/// Report the kinds of entries we would create, for diagnostics.
pub fn list(data: &[u8]) -> Vec<(String, InodeKind, usize)> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset + BLOCK <= data.len() {
        let header = Header(&data[offset..offset + BLOCK]);
        if header.is_zero() {
            break;
        }
        let size = header.size();
        let kind = match header.typeflag() {
            b'5' => InodeKind::Dir,
            b'2' => InodeKind::Symlink,
            _ => InodeKind::File,
        };
        out.push((header.name(), kind, size));
        offset += BLOCK + (size + BLOCK - 1) / BLOCK * BLOCK;
    }
    out
}

/// Unused but handy for callers wanting the root after extraction.
pub fn root() -> InodeRef {
    super::root().clone()
}
