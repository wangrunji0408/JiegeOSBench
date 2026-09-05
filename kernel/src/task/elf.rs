//! ELF64 parsing and loading into an address space.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::abi::*;
use crate::config::PAGE_SIZE;
use crate::fs::file::File;
use crate::mm::addrspace::{page_up, AddressSpace, Prot, Vma};
use crate::mm::uaccess::copy_to_user_mm;

pub const PT_LOAD: u32 = 1;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_STACK: u32 = 0x6474e551;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_RISCV: u16 = 243;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Ehdr {
    pub ident: [u8; 16],
    pub e_type: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u64,
    pub phoff: u64,
    pub shoff: u64,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Phdr {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub paddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

pub struct ElfInfo {
    pub ehdr: Ehdr,
    pub phdrs: Vec<Phdr>,
    pub interp: Option<String>,
}

fn read_exact(file: &File, off: u64, buf: &mut [u8]) -> Result<(), i32> {
    let mut done = 0;
    while done < buf.len() {
        let n = file.pread(&mut buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(ENOEXEC);
        }
        done += n;
    }
    Ok(())
}

pub fn parse(file: &File) -> Result<ElfInfo, i32> {
    let mut hdr = [0u8; core::mem::size_of::<Ehdr>()];
    read_exact(file, 0, &mut hdr)?;
    let ehdr: Ehdr = unsafe { core::ptr::read_unaligned(hdr.as_ptr() as *const Ehdr) };
    if &ehdr.ident[0..4] != b"\x7fELF" || ehdr.ident[4] != 2 || ehdr.machine != EM_RISCV {
        return Err(ENOEXEC);
    }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(ENOEXEC);
    }
    if ehdr.phentsize as usize != core::mem::size_of::<Phdr>() || ehdr.phnum > 256 {
        return Err(ENOEXEC);
    }
    let mut phdrs = Vec::with_capacity(ehdr.phnum as usize);
    let mut buf = [0u8; core::mem::size_of::<Phdr>()];
    for i in 0..ehdr.phnum as u64 {
        read_exact(file, ehdr.phoff + i * ehdr.phentsize as u64, &mut buf)?;
        phdrs.push(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Phdr) });
    }
    let mut interp = None;
    for ph in &phdrs {
        if ph.p_type == PT_INTERP && ph.filesz > 0 && ph.filesz < 4096 {
            let mut s = alloc::vec![0u8; ph.filesz as usize];
            read_exact(file, ph.offset, &mut s)?;
            while s.last() == Some(&0) {
                s.pop();
            }
            interp = Some(String::from_utf8_lossy(&s).into_owned());
        }
    }
    Ok(ElfInfo { ehdr, phdrs, interp })
}

pub struct Loaded {
    pub base: usize,
    pub entry: usize,
    pub phdr_addr: usize,
    pub end: usize,
}

/// Map all PT_LOAD segments of `info` into `mm`. For ET_DYN, `base_hint` is used.
pub fn load(
    mm: &Arc<crate::sync::SpinLock<AddressSpace>>,
    file: &Arc<File>,
    info: &ElfInfo,
    base_hint: usize,
) -> Result<Loaded, i32> {
    let base = if info.ehdr.e_type == ET_DYN { base_hint } else { 0 };
    let mut end = 0usize;
    let mut phdr_addr = 0usize;
    let mut zero_tails: Vec<(usize, usize)> = Vec::new();
    {
        let mut a = mm.lock();
        for ph in &info.phdrs {
            if ph.p_type == PT_PHDR {
                phdr_addr = base + ph.vaddr as usize;
            }
            if ph.p_type != PT_LOAD || ph.memsz == 0 {
                continue;
            }
            let vaddr = base + ph.vaddr as usize;
            let mut prot = Prot::empty();
            if ph.flags & PF_R != 0 {
                prot |= Prot::R;
            }
            if ph.flags & PF_W != 0 {
                prot |= Prot::W;
            }
            if ph.flags & PF_X != 0 {
                prot |= Prot::X;
            }
            let seg_start = vaddr & !(PAGE_SIZE - 1);
            let file_end = vaddr + ph.filesz as usize;
            let mem_end = vaddr + ph.memsz as usize;
            let file_end_page = page_up(file_end);
            let mem_end_page = page_up(mem_end);
            if (vaddr as u64 % PAGE_SIZE as u64) != (ph.offset % PAGE_SIZE as u64) {
                return Err(ENOEXEC);
            }
            let file_off = ph.offset & !(PAGE_SIZE as u64 - 1);
            if ph.filesz > 0 {
                // Unmap anything already there (overlapping segments are unusual)
                a.munmap(seg_start, file_end_page - seg_start);
                a.insert_vma(Vma {
                    start: seg_start,
                    end: file_end_page,
                    prot,
                    shared: false,
                    file: Some((file.clone(), file_off)),
                    grows_down: false,
                });
            }
            if mem_end_page > file_end_page {
                let anon_start = if ph.filesz > 0 { file_end_page } else { seg_start };
                a.munmap(anon_start, mem_end_page - anon_start);
                a.insert_vma(Vma {
                    start: anon_start,
                    end: mem_end_page,
                    prot,
                    shared: false,
                    file: None,
                    grows_down: false,
                });
            }
            if ph.memsz > ph.filesz && file_end % PAGE_SIZE != 0 {
                zero_tails.push((file_end, file_end_page - file_end));
            }
            end = end.max(mem_end_page);
        }
    }
    // Zero the part of the last file page beyond filesz (bss start).
    for (addr, len) in zero_tails {
        let zeros = alloc::vec![0u8; len];
        // Ignore failures on read-only segments.
        let _ = copy_to_user_mm(mm, addr, &zeros);
    }
    if phdr_addr == 0 {
        // Find the segment containing the program headers.
        for ph in &info.phdrs {
            if ph.p_type == PT_LOAD && ph.offset <= info.ehdr.phoff && info.ehdr.phoff < ph.offset + ph.filesz {
                phdr_addr = base + (ph.vaddr + (info.ehdr.phoff - ph.offset)) as usize;
                break;
            }
        }
    }
    Ok(Loaded { base, entry: base + info.ehdr.entry as usize, phdr_addr, end })
}
