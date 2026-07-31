//! Minimal Flattened Device Tree parser (big-endian), used to find RAM size,
//! timer frequency and kernel bootargs on QEMU virt.

use crate::kprintln;
use alloc::string::{String, ToString};

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

pub struct DtbInfo {
    pub ram_base: usize,
    pub ram_size: usize,
    pub timebase: u64,
    pub bootargs: Option<&'static str>,
}

fn be32(p: *const u8) -> u32 {
    unsafe {
        u32::from_be_bytes([
            *p,
            *p.add(1),
            *p.add(2),
            *p.add(3),
        ])
    }
}

fn be64(p: *const u8) -> u64 {
    unsafe {
        u64::from_be_bytes([
            *p, *p.add(1), *p.add(2), *p.add(3),
            *p.add(4), *p.add(5), *p.add(6), *p.add(7),
        ])
    }
}

fn cstr(p: *const u8) -> &'static str {
    unsafe {
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, len))
    }
}

pub fn parse(dtb: usize) -> DtbInfo {
    let mut info = DtbInfo {
        ram_base: 0x8000_0000,
        ram_size: 512 * 1024 * 1024,
        timebase: 10_000_000,
        bootargs: None,
    };
    let base = dtb as *const u8;
    let magic = be32(base);
    if magic != FDT_MAGIC {
        kprintln!("[dtb] invalid magic {:#x}, using defaults", magic);
        return info;
    }
    let (totalsize, off_struct, off_strings, size_strings, strings, mut p) = unsafe {
        let totalsize = be32(base.add(4)) as usize;
        let off_struct = be32(base.add(8)) as usize;
        let off_strings = be32(base.add(12)) as usize;
        let size_strings = be32(base.add(20)) as usize;
        let strings = base.add(off_strings);
        let p = base.add(off_struct);
        (totalsize, off_struct, off_strings, size_strings, strings, p)
    };

    // root address/size cells
    let mut addr_cells = 2u32;
    let mut size_cells = 1u32;
    let mut depth = 0i32;
    let mut cur_node = String::new();
    let mut found_mem = false;
    let mut found_cpu = false;
    let mut found_chosen = false;

    unsafe {
    loop {
        let tok = be32(p);
        p = p.add(4);
        match tok {
            FDT_BEGIN_NODE => {
                let name = cstr(p);
                let len = name.len() + 1;
                p = p.add((len + 3) & !3);
                cur_node = name.to_string();
                depth += 1;
            }
            FDT_END_NODE => {
                depth -= 1;
                cur_node.clear();
            }
            FDT_PROP => {
                let len = be32(p) as usize;
                let nameoff = be32(p.add(4)) as usize;
                let data = p.add(8);
                let name = cstr(strings.add(nameoff));
                if depth == 0 {
                    if name == "#address-cells" {
                        addr_cells = be32(data);
                    } else if name == "#size-cells" {
                        size_cells = be32(data);
                    }
                } else if depth == 1 {
                    let node = cur_node.as_str();
                    if name == "reg" && node.starts_with("memory") {
                        // root cells apply
                        let mut off = 0usize;
                        let mut addr = 0u64;
                        for _ in 0..addr_cells as usize {
                            addr = (addr << 32) | be32(data.add(off)) as u64;
                            off += 4;
                        }
                        let mut size = 0u64;
                        for _ in 0..size_cells as usize {
                            size = (size << 32) | be32(data.add(off)) as u64;
                            off += 4;
                        }
                        info.ram_base = addr as usize;
                        info.ram_size = size as usize;
                        found_mem = true;
                    } else if name == "timebase-frequency" && node == "cpus" {
                        info.timebase = be32(data) as u64;
                        found_cpu = true;
                    } else if name == "bootargs" && node == "chosen" {
                        info.bootargs = Some(cstr(data));
                        found_chosen = true;
                    }
                }
                p = p.add(8 + ((len + 3) & !3));
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => {
                kprintln!("[dtb] unknown token {:#x}", tok);
                break;
            }
        }
        if found_mem && found_cpu && found_chosen {
            break;
        }
    }
    }
    info
}
