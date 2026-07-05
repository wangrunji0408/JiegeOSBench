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
        let mut pa = crate::mm::frame::FRAME_ALLOCATOR
            .alloc_zeroed()
            .ok_or("OOM loading ELF")?;
        let mut va = start;
        while va < end {
            // 每页分配新帧
            if va != start {
                pa = crate::mm::frame::FRAME_ALLOCATOR
                    .alloc_zeroed()
                    .ok_or("OOM loading ELF")?;
            }
            // 拷贝文件内容到该页
            let page_dst = pa as *mut u8;
            let off_in_seg = va as isize - start as isize; // 相对段页起始
            // 实际上要按 vaddr 偏移计算文件来源
            let file_off = (va as isize - p_vaddr as isize) + p_offset as isize;
            let file_len = if file_off >= 0 && (file_off as usize) < p_offset + p_filesz {
                let remain = p_filesz.saturating_sub(file_off as usize - p_offset);
                remain.min(PAGE_SIZE - (p_vaddr & (PAGE_SIZE - 1)))
            } else {
                0
            };
            // 简化：直接按页内偏移拷贝文件数据
            let page_off = p_vaddr & (PAGE_SIZE - 1); // 段起始在页内偏移
            let _ = (off_in_seg, page_off);
            // 计算该页应填充的字节范围
            let copy_start = if va == start { p_vaddr & (PAGE_SIZE - 1) } else { 0 };
            let copy_end = if va + PAGE_SIZE >= p_vaddr + p_filesz {
                (p_vaddr + p_filesz - va).min(PAGE_SIZE)
            } else {
                PAGE_SIZE
            };
            if copy_end > copy_start {
                let src_off = p_offset + (va + copy_start - p_vaddr);
                if src_off + (copy_end - copy_start) <= elf.len() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            elf.as_ptr().add(src_off),
                            page_dst.add(copy_start),
                            copy_end - copy_start,
                        );
                    }
                }
            }
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
