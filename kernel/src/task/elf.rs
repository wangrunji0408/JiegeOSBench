//! ELF64 loader for RISC-V user binaries (static, PIE and dynamically linked).

use crate::mm::address::*;
use crate::mm::page_table::*;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const EM_RISCV: u16 = 243;

#[derive(Clone, Copy, Debug)]
pub struct Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

#[derive(Clone, Debug)]
pub struct ElfImage {
    pub entry: usize,
    pub base: usize,
    pub phdr: usize,
    pub phnum: usize,
    pub phent: usize,
    pub interp: Option<alloc::string::String>,
    pub is_dyn: bool,
    pub max_vaddr: usize,
}

fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn rd_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

pub fn parse(data: &[u8]) -> Result<ElfImage, &'static str> {
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return Err("not an ELF file");
    }
    if data[4] != 2 || data[5] != 1 {
        return Err("not a 64-bit little-endian ELF");
    }
    let e_type = rd_u16(data, 16);
    if rd_u16(data, 18) != EM_RISCV {
        return Err("not a RISC-V binary");
    }
    let entry = rd_u64(data, 24) as usize;
    let phoff = rd_u64(data, 32) as usize;
    let phentsize = rd_u16(data, 54) as usize;
    let phnum = rd_u16(data, 56) as usize;
    if phoff + phnum * phentsize > data.len() {
        return Err("program headers out of range");
    }
    let mut interp = None;
    let mut max_vaddr = 0usize;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        let p_type = rd_u32(data, o);
        if p_type == PT_INTERP {
            let off = rd_u64(data, o + 8) as usize;
            let sz = rd_u64(data, o + 32) as usize;
            if off + sz <= data.len() {
                let s = &data[off..off + sz];
                let s = s.split(|&b| b == 0).next().unwrap_or(&[]);
                interp = Some(alloc::string::String::from_utf8_lossy(s).into_owned());
            }
        }
        if p_type == PT_LOAD {
            let va = rd_u64(data, o + 16) as usize;
            let memsz = rd_u64(data, o + 40) as usize;
            max_vaddr = max_vaddr.max(va + memsz);
        }
    }
    Ok(ElfImage {
        entry,
        base: 0,
        phdr: 0,
        phnum,
        phent: phentsize,
        interp,
        is_dyn: e_type == 3,
        max_vaddr,
    })
}

pub fn phdrs(data: &[u8], phoff: usize, phnum: usize, phentsize: usize) -> alloc::vec::Vec<Phdr> {
    let mut v = alloc::vec::Vec::new();
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        v.push(Phdr {
            p_type: rd_u32(data, o),
            p_flags: rd_u32(data, o + 4),
            p_offset: rd_u64(data, o + 8),
            p_vaddr: rd_u64(data, o + 16),
            p_filesz: rd_u64(data, o + 32),
            p_memsz: rd_u64(data, o + 40),
            p_align: rd_u64(data, o + 48),
        });
    }
    v
}

pub fn phoff(data: &[u8]) -> usize {
    rd_u64(data, 32) as usize
}

/// Map one ELF segment eagerly: allocate frames, copy the file bytes and zero the bss.
pub fn load_segment(
    pt: &mut PageTable,
    data: &[u8],
    ph: &Phdr,
    base: usize,
) -> Result<usize, &'static str> {
    if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
        return Ok(0);
    }
    let vaddr = (ph.p_vaddr as usize).wrapping_add(base);
    let start = page_align_down(vaddr);
    let end = page_align_up(vaddr + ph.p_memsz as usize);
    let mut flags = PTE_U | PTE_A | PTE_D;
    if ph.p_flags & PF_R != 0 {
        flags |= PTE_R;
    }
    if ph.p_flags & PF_W != 0 {
        flags |= PTE_W;
    }
    if ph.p_flags & PF_X != 0 {
        flags |= PTE_X;
    }
    let file_start = ph.p_offset as usize;
    let file_end = file_start + ph.p_filesz as usize;
    let mut va = start;
    while va < end {
        let frame = crate::mm::frame::alloc_frame().ok_or("out of memory loading ELF")?;
        // copy the file-backed part of this page
        let page_vaddr = va;
        let src_lo = page_vaddr.max(vaddr);
        let src_hi = (page_vaddr + PAGE_SIZE).min(vaddr + ph.p_filesz as usize);
        if src_hi > src_lo {
            let dst_off = src_lo - page_vaddr;
            let fo = file_start + (src_lo - vaddr);
            let n = src_hi - src_lo;
            if fo + n <= data.len() && file_end <= data.len() + PAGE_SIZE {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr().add(fo),
                        (frame + dst_off) as *mut u8,
                        n,
                    );
                }
            }
        }
        pt.map(va, frame, flags).map_err(|_| "page already mapped")?;
        va += PAGE_SIZE;
    }
    Ok(end)
}
