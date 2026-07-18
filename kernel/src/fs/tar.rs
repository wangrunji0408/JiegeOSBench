//! Minimal USTAR reader used to populate the in-memory rootfs from a tar
//! archive embedded in the kernel binary at build time.

use super::tmpfs::{self, Inode};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

const BLOCK: usize = 512;

fn octal(field: &[u8]) -> usize {
    let s = core::str::from_utf8(field).unwrap_or("");
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() {
        return 0;
    }
    usize::from_str_radix(s, 8).unwrap_or(0)
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

pub fn extract(archive: &[u8], _root: &Arc<Inode>) {
    let mut off = 0usize;
    while off + BLOCK <= archive.len() {
        let header = &archive[off..off + BLOCK];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let name = cstr(&header[0..100]);
        let size = octal(&header[124..136]);
        let typeflag = header[156];
        let linkname = cstr(&header[157..257]);
        off += BLOCK;

        let path = name.trim_start_matches("./").trim_end_matches('/');
        let data_start = off;
        let data_len = size;
        off += (data_len + BLOCK - 1) / BLOCK * BLOCK;

        if path.is_empty() {
            continue;
        }
        match typeflag {
            b'5' => {
                tmpfs::make_dirs_absolute(path);
            }
            b'2' => {
                tmpfs::insert_absolute(path, Arc::new(Inode::Symlink(linkname)));
            }
            b'0' | 0 => {
                let data = archive[data_start..data_start + data_len].to_vec();
                tmpfs::insert_absolute(path, Inode::new_file(data));
            }
            _ => {}
        }
    }
}
