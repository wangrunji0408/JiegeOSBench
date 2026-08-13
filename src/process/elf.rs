//! ELF64 (RISC-V) parser and loader.

use crate::memory::frame;
use crate::memory::page_table::PageTable;
use crate::memory::PAGE_SIZE;

pub const EM_RISCV: u16 = 243;
pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;

pub const PT_LOAD: u32 = 1;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

#[derive(Debug)]
pub struct ElfError;

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off], b[off + 1], b[off + 2], b[off + 3], b[off + 4], b[off + 5], b[off + 6], b[off + 7],
    ])
}

pub struct LoadedElf {
    /// Load base (0 for ET_EXEC, else the base virtual address chosen).
    pub base: usize,
    pub entry: usize,
    pub phdr: usize,
    pub phentsize: usize,
    pub phnum: usize,
    /// Highest mapped address (+1) so callers can place the stack below it.
    pub max_va: usize,
}

/// Load an ELF image into `pt`. Returns metadata needed for auxv/entry.
pub fn load(pt: &mut PageTable, data: &[u8]) -> Result<LoadedElf, ElfError> {
    if data.len() < 64 {
        return Err(ElfError);
    }
    // magic + class + data + version
    if &data[0..4] != b"\x7fELF" || data[4] != 2 || data[5] != 1 || data[6] != 1 {
        return Err(ElfError);
    }
    let e_type = u16_at(data, 16);
    let e_machine = u16_at(data, 18);
    if e_machine != EM_RISCV {
        return Err(ElfError);
    }
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(ElfError);
    }
    let e_entry = u64_at(data, 24) as usize;
    let e_phoff = u64_at(data, 32) as usize;
    let e_phentsize = u16_at(data, 54) as usize;
    let e_phnum = u16_at(data, 56) as usize;
    if e_phentsize < 56 {
        return Err(ElfError);
    }

    // Choose a load base. For ET_DYN (PIE) pick a fixed base; segments are
    // mapped at base + p_vaddr. For ET_EXEC base is 0 (absolute addresses).
    let base = if e_type == ET_DYN { 0x1_0000 } else { 0 };

    let mut max_va = 0usize;
    let mut first_load_vaddr: Option<usize> = None;
    let mut phdr_vaddr: Option<usize> = None;
    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        if ph + 56 > data.len() {
            return Err(ElfError);
        }
        let p_type = u32_at(data, ph);
        let p_vaddr = u64_at(data, ph + 16) as usize;
        if p_type == 6 {
            // PT_PHDR: virtual address of the program header table
            phdr_vaddr = Some(p_vaddr);
            continue;
        }
        if p_type != PT_LOAD {
            continue;
        }
        if first_load_vaddr.is_none() {
            first_load_vaddr = Some(p_vaddr);
        }
        let p_flags = u32_at(data, ph + 4);
        let p_offset = u64_at(data, ph + 8) as usize;
        let p_filesz = u64_at(data, ph + 32) as usize;
        let p_memsz = u64_at(data, ph + 40) as usize;

        let mut prot = 0usize;
        if p_flags & PF_R != 0 { prot |= crate::memory::page_table::PTE_R; }
        if p_flags & PF_W != 0 { prot |= crate::memory::page_table::PTE_W; }
        if p_flags & PF_X != 0 { prot |= crate::memory::page_table::PTE_X; }
        prot |= crate::memory::page_table::PTE_U | crate::memory::page_table::PTE_A | crate::memory::page_table::PTE_D;

        let va = base + p_vaddr;
        map_segment(pt, va, data, p_offset, p_filesz, p_memsz, prot);

        let end = crate::memory::frame::align_up(va + p_memsz, PAGE_SIZE);
        if end > max_va {
            max_va = end;
        }
    }

    // The program headers are mapped either at the PT_PHDR virtual address or,
    // for the usual layout, at base + first-load-vaddr + e_phoff.
    let phdr = match phdr_vaddr {
        Some(v) => base + v,
        None => base + first_load_vaddr.unwrap_or(0) + e_phoff,
    };

    Ok(LoadedElf {
        base,
        entry: base + e_entry,
        phdr,
        phentsize: e_phentsize,
        phnum: e_phnum,
        max_va,
    })
}

/// Map one PT_LOAD segment: allocate frames, copy file bytes, zero the rest.
fn map_segment(
    pt: &mut PageTable,
    va: usize,
    data: &[u8],
    offset: usize,
    filesz: usize,
    memsz: usize,
    flags: usize,
) {
    let start = frame::align_down(va, PAGE_SIZE);
    let end = frame::align_up(va + memsz, PAGE_SIZE);

    let mut cur = start;
    while cur < end {
        let f = frame::alloc().expect("out of frames loading ELF");
        pt.map(cur, f.0, flags);

        // Copy the portion of the file that lands in this page.
        let page_start = cur;
        let page_end = cur + PAGE_SIZE;
        let d_start = page_start.max(va);
        let d_end = page_end.min(va + memsz);
        if d_start < d_end {
            let src_start = offset + (d_start - va);
            let src_end = offset + (d_end - va);
            // only copy up to filesz (rest is .bss, already zeroed)
            let copy_end = src_end.min(offset + filesz);
            if src_start < copy_end {
                let len = copy_end - src_start;
                let dst = f.0 + (d_start - page_start);
                unsafe {
                    core::ptr::copy_nonoverlapping(data.as_ptr().add(src_start), dst as *mut u8, len);
                }
            }
        }
        cur += PAGE_SIZE;
    }
}
