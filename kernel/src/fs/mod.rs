pub mod ramfs;
pub mod cpio;

pub use ramfs::{FileSystem, FS};

use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;

pub fn init() {
    // 初始化内存文件系统
    ramfs::init();
    // 解包内嵌的CPIO initramfs
    let initramfs_data = get_initramfs_data();
    if !initramfs_data.is_empty() {
        cpio::unpack(initramfs_data, &FS);
        println!("[fs] Initramfs unpacked");
    } else {
        println!("[fs] No initramfs data");
    }
}

fn get_initramfs_data() -> &'static [u8] {
    // QEMU把initramfs放在物理地址0x88200000（通过DTB传递）
    // 我们先尝试固定地址，然后扫描
    const INITRD_PHYS_ADDR: usize = 0x88200000;
    const INITRD_MAX_SIZE: usize = 128 * 1024 * 1024; // 128MB

    let initrd_va = crate::utils::phys_to_virt(INITRD_PHYS_ADDR);

    // 检查是否有CPIO magic
    let magic = unsafe { core::slice::from_raw_parts(initrd_va as *const u8, 6) };
    if magic == b"070701" || magic == b"070702" {
        // 找到了initramfs，扫描其大小
        let end_va = scan_cpio_end(initrd_va, initrd_va + INITRD_MAX_SIZE);
        let len = end_va - initrd_va;
        println!("[fs] CPIO initramfs at phys {:#x}, size {}KB",
            INITRD_PHYS_ADDR, len / 1024);
        return unsafe { core::slice::from_raw_parts(initrd_va as *const u8, len) };
    }

    // 如果固定地址没有，扫描内存
    scan_for_cpio()
}

fn scan_cpio_end(start_va: usize, max_end_va: usize) -> usize {
    let mut pos = start_va;
    loop {
        if pos + 110 > max_end_va { break; }
        let header = unsafe { core::slice::from_raw_parts(pos as *const u8, 6) };
        if header != b"070701" && header != b"070702" { break; }

        let h = unsafe { core::slice::from_raw_parts(pos as *const u8, 110) };
        let namesize = parse_hex_cpio_bytes(&h[94..102]) as usize;
        let filesize = parse_hex_cpio_bytes(&h[54..62]) as usize;

        // 检查是否是TRAILER
        let header_end = pos + 110;
        let name_end = header_end + namesize;
        let aligned_name_end = align4(110 + namesize) + pos;
        let aligned_data_end = aligned_name_end + align4(filesize);

        if namesize >= 11 && namesize <= 200 {
            let name = unsafe {
                core::slice::from_raw_parts(header_end as *const u8, namesize.min(11))
            };
            if name.starts_with(b"TRAILER!!!") {
                return aligned_data_end;
            }
        }

        pos = aligned_data_end;
    }
    pos
}

fn align4(n: usize) -> usize { (n + 3) & !3 }

fn parse_hex_cpio_bytes(s: &[u8]) -> u64 {
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

fn scan_for_cpio() -> &'static [u8] {
    // 扫描物理内存寻找CPIO magic "070701"
    const CPIO_MAGIC: &[u8] = b"070701";
    extern "C" {
        fn ekernel();
    }
    let kernel_end_pa = crate::utils::virt_to_phys(ekernel as usize);
    let scan_start = (kernel_end_pa + 0xFFF) & !0xFFF;
    let scan_end = crate::config::MEMORY_END - 4096;

    let scan_start_va = crate::utils::phys_to_virt(scan_start);
    let scan_end_va = crate::utils::phys_to_virt(scan_end);

    let mut addr = scan_start_va;
    while addr + 6 <= scan_end_va {
        let slice = unsafe { core::slice::from_raw_parts(addr as *const u8, 6) };
        if slice == CPIO_MAGIC {
            let end = scan_cpio_end(addr, scan_end_va);
            let len = end - addr;
            if len > 1024 { // 至少1KB
                println!("[fs] Found CPIO initramfs at {:#x}, size {}KB",
                    addr, len / 1024);
                return unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
            }
        }
        addr += 4096;
    }

    println!("[fs] No CPIO initramfs found in memory");
    &[]
}

/// 文件类型
#[derive(Clone, Debug, PartialEq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

/// 文件元数据
#[derive(Clone, Debug)]
pub struct FileStat {
    pub size: usize,
    pub file_type: FileType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub ino: u64,
    pub rdev: u64,
}
