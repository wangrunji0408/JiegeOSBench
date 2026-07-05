//! 最小 ELF64 加载器：解析 PT_LOAD 段，映射到进程地址空间并填充内容。

use crate::mm::page_table::{PageTable, PTE_R, PTE_W, PTE_X, PTE_U, PTE_A, PTE_D};
use crate::mm::address::PAGE_SIZE;

const ELF_MAGIC: u32 = 0x464C457F; // "\x7fELF"
const PT_LOAD: u32 = 1;

#[repr(C)]
pub struct ElfHeader {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C)]
pub struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

pub struct LoadedElf {
    pub entry: usize,
    pub brk_start: usize, // 可用于 brk 的起始（最大段尾，页对齐）
    pub phdr: usize,     // 程序头表虚拟地址
    pub phnum: usize,
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn read_u64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}
fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// 加载 ELF 到给定页表，返回入口与 brk 起点。
/// 用户栈顶由调用方另行映射。
pub fn load_elf(elf: &[u8], pt: &PageTable) -> Result<LoadedElf, &'static str> {
    if elf.len() < 64 || read_u32(elf, 0) != ELF_MAGIC {
        return Err("bad ELF magic");
    }
    let e_entry = read_u64(elf, 24) as usize;
    let e_phoff = read_u64(elf, 32) as usize;
    let e_phentsize = read_u16(elf, 54) as usize;
    let e_phnum = read_u16(elf, 56) as usize;

    let mut max_end: usize = 0;

    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        if ph + 56 > elf.len() {
            break;
        }
        let p_type = read_u32(elf, ph);
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = read_u32(elf, ph + 4);
        let p_offset = read_u64(elf, ph + 8) as usize;
        let p_vaddr = read_u64(elf, ph + 16) as usize;
        let p_filesz = read_u64(elf, ph + 32) as usize;
        let p_memsz = read_u64(elf, ph + 40) as usize;

        let mut perm = PTE_U | PTE_A | PTE_D;
        if p_flags & 4 != 0 {
            perm |= PTE_R;
        }
        if p_flags & 2 != 0 {
            perm |= PTE_W;
        }
        if p_flags & 1 != 0 {
            perm |= PTE_X;
        }

        // 按 4KB 映射 [vaddr, vaddr+memsz)
        let start = p_vaddr & !(PAGE_SIZE - 1);
        let end = (p_vaddr + p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut va = start;
        while va < end {
            let pa = crate::mm::frame::FRAME_ALLOCATOR
                .alloc_zeroed()
                .ok_or("OOM loading ELF")?;
            // 该页与文件数据 [p_vaddr, p_vaddr+p_filesz) 的交集
            let ov_start = va.max(p_vaddr);
            let ov_end = (va + PAGE_SIZE).min(p_vaddr + p_filesz);
            if ov_end > ov_start {
                let dst_off = ov_start - va; // 页内偏移
                let src_off = p_offset + (ov_start - p_vaddr);
                if src_off + (ov_end - ov_start) <= elf.len() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            elf.as_ptr().add(src_off),
                            (pa as *mut u8).add(dst_off),
                            ov_end - ov_start,
                        );
                    }
                }
            }
            // memsz > filesz 的部分（bss）已是零（alloc_zeroed）
            pt.map_page(va, pa, perm);
            va += PAGE_SIZE;
        }

        if end > max_end {
            max_end = end;
        }
    }

    Ok(LoadedElf {
        entry: e_entry,
        brk_start: max_end,
    })
}
