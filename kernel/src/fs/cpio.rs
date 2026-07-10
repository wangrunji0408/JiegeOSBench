/// CPIO newc格式解析器
/// 用于解包initramfs

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::ramfs::FileSystem;

// CPIO newc 头部（110字节）
const CPIO_MAGIC: &[u8] = b"070701";
const HEADER_SIZE: usize = 110;

#[repr(C)]
struct CpioHeader {
    // 字段都是ASCII十六进制
    magic: [u8; 6],
    ino: [u8; 8],
    mode: [u8; 8],
    uid: [u8; 8],
    gid: [u8; 8],
    nlink: [u8; 8],
    mtime: [u8; 8],
    filesize: [u8; 8],
    devmajor: [u8; 8],
    devminor: [u8; 8],
    rdevmajor: [u8; 8],
    rdevminor: [u8; 8],
    namesize: [u8; 8],
    check: [u8; 8],
}

fn parse_hex(s: &[u8]) -> u64 {
    let mut result = 0u64;
    for &b in s {
        result = result * 16 + match b {
            b'0'..=b'9' => (b - b'0') as u64,
            b'a'..=b'f' => (b - b'a' + 10) as u64,
            b'A'..=b'F' => (b - b'A' + 10) as u64,
            _ => 0,
        };
    }
    result
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

pub fn unpack(data: &[u8], fs: &FileSystem) {
    let mut pos = 0;

    while pos + HEADER_SIZE <= data.len() {
        // 检查magic
        if &data[pos..pos+6] != CPIO_MAGIC {
            break;
        }

        let h = &data[pos..pos+HEADER_SIZE];
        let mode = parse_hex(&h[14..22]) as u32;
        let uid = parse_hex(&h[22..30]) as u32;
        let gid = parse_hex(&h[30..38]) as u32;
        let filesize = parse_hex(&h[54..62]) as usize;
        let devmajor = parse_hex(&h[62..70]) as u32;
        let devminor = parse_hex(&h[70..78]) as u32;
        let rdevmajor = parse_hex(&h[78..86]) as u32;
        let rdevminor = parse_hex(&h[86..94]) as u32;
        let namesize = parse_hex(&h[94..102]) as usize;

        let start_pos = pos; // 记录header开始位置
        pos += HEADER_SIZE;

        // 读取文件名
        if pos + namesize > data.len() { break; }
        let name_bytes = &data[pos..pos+namesize-1]; // 去掉末尾的\0
        let name = core::str::from_utf8(name_bytes).unwrap_or("?");

        // name结束后对齐到4字节（从header开始计算）
        let name_end = HEADER_SIZE + namesize;
        let name_aligned = (name_end + 3) & !3;
        pos = start_pos + name_aligned;

        // 跳过TRAILER
        if name == "TRAILER!!!" {
            break;
        }

        // 路径标准化
        let path = if name.starts_with("./") {
            format!("/{}", &name[2..])
        } else if !name.starts_with('/') {
            format!("/{}", name)
        } else {
            name.to_string()
        };

        // file_type是mode的高4位（mode >> 12）
        let file_type = mode >> 12;
        const S_IFDIR: u32 = 4;   // 0o040000 >> 12 = 4
        const S_IFREG: u32 = 8;   // 0o100000 >> 12 = 8
        const S_IFLNK: u32 = 10;  // 0o120000 >> 12 = 10 (0xA)
        const S_IFCHR: u32 = 2;   // 0o020000 >> 12 = 2
        const S_IFBLK: u32 = 6;   // 0o060000 >> 12 = 6
        const S_IFIFO: u32 = 1;   // 0o010000 >> 12 = 1
        const S_IFSOCK: u32 = 12; // 0o140000 >> 12 = 12

        match file_type {
            ft if ft == S_IFDIR => {
                fs.mkdir_p(&path);
            }
            ft if ft == S_IFREG => {
                if pos + filesize > data.len() { break; }
                let content = data[pos..pos+filesize].to_vec();
                let perm = mode & 0o7777;
                fs.create_file(&path, content, perm);
            }
            ft if ft == S_IFLNK => {
                if pos + filesize > data.len() { break; }
                let target = core::str::from_utf8(&data[pos..pos+filesize])
                    .unwrap_or("").to_string();
                fs.create_symlink(&path, &target, mode & 0o7777);
            }
            ft if ft == S_IFCHR => {
                fs.create_char_dev(&path, rdevmajor, rdevminor);
            }
            _ => {
                // 其他类型忽略
            }
        }

        pos += align4(filesize);
    }
}
