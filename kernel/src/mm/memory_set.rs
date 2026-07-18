//! Address spaces: a `MemorySet` is a page table plus the list of mapped
//! areas needed to reconstruct/tear it down.

use super::address::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::frame_allocator::{frame_alloc, FrameTracker};
use super::page_table::{PTEFlags, PageTable};
use crate::config::{MEMORY_END, MMIO, PAGE_SIZE};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::arch::asm;

bitflags! {
    #[derive(Copy, Clone, Debug)]
    pub struct MapPermission: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

impl From<MapPermission> for PTEFlags {
    fn from(p: MapPermission) -> Self {
        PTEFlags::from_bits_truncate(p.bits())
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MapType {
    Identical,
    Framed,
}

pub struct MapArea {
    pub vpn_start: VirtPageNum,
    pub vpn_end: VirtPageNum,
    pub data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    pub map_type: MapType,
    pub perm: MapPermission,
    /// Page offset of the area's true start address, for ELF segments
    /// whose `p_vaddr` isn't itself page-aligned (extremely common: only
    /// the *first* segment of an object starts on a page boundary).
    /// `copy_data` needs this so segment content lands at the exact
    /// virtual address the ELF (and its relocations/GOT) expect, not at
    /// the containing page's start.
    data_offset: usize,
}

impl MapArea {
    pub fn new(start_va: VirtAddr, end_va: VirtAddr, map_type: MapType, perm: MapPermission) -> Self {
        Self {
            vpn_start: start_va.floor(),
            vpn_end: end_va.ceil(),
            data_frames: BTreeMap::new(),
            map_type,
            perm,
            data_offset: start_va.page_offset(),
        }
    }

    pub fn from_existing(vpn_start: VirtPageNum, vpn_end: VirtPageNum, perm: MapPermission) -> Self {
        Self {
            vpn_start,
            vpn_end,
            data_frames: BTreeMap::new(),
            map_type: MapType::Framed,
            perm,
            data_offset: 0,
        }
    }

    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn = match self.map_type {
            MapType::Identical => PhysPageNum(vpn.0),
            MapType::Framed => {
                let frame = frame_alloc().expect("out of physical memory");
                let ppn = frame.ppn;
                self.data_frames.insert(vpn, frame);
                ppn
            }
        };
        page_table.map(vpn, ppn, PTEFlags::from(self.perm));
    }

    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        if self.map_type == MapType::Framed {
            self.data_frames.remove(&vpn);
        }
        page_table.unmap(vpn);
    }

    pub fn map(&mut self, page_table: &mut PageTable) {
        let mut vpn = self.vpn_start;
        while vpn.0 < self.vpn_end.0 {
            self.map_one(page_table, vpn);
            vpn.0 += 1;
        }
    }

    pub fn unmap(&mut self, page_table: &mut PageTable) {
        let mut vpn = self.vpn_start;
        while vpn.0 < self.vpn_end.0 {
            self.unmap_one(page_table, vpn);
            vpn.0 += 1;
        }
    }

    /// Copy raw bytes into this area starting at its true start address
    /// (see `data_offset`), as used when loading ELF segment contents.
    /// `data` may be shorter than the area (the remainder is left zeroed
    /// by the fresh frame allocation).
    pub fn copy_data(&mut self, page_table: &PageTable, data: &[u8]) {
        let mut start = 0;
        let mut vpn = self.vpn_start;
        let mut page_off = self.data_offset;
        loop {
            let src = &data[start..data.len().min(start + PAGE_SIZE - page_off)];
            let ppn = page_table.translate(vpn).unwrap().ppn();
            let dst = &mut ppn.as_bytes()[page_off..page_off + src.len()];
            dst.copy_from_slice(src);
            start += src.len();
            if start >= data.len() {
                break;
            }
            vpn.0 += 1;
            page_off = 0;
        }
    }
}

pub struct MemorySet {
    pub page_table: PageTable,
    pub areas: Vec<MapArea>,
    /// Bump allocator for anonymous/file-backed `mmap` regions with no
    /// caller-specified address; never reclaimed on `munmap`, which is a
    /// fine trade for this workload's modest mmap traffic.
    pub mmap_top: usize,
}

impl MemorySet {
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
            mmap_top: crate::config::MMAP_BASE,
        }
    }

    /// Deep-copy an existing (user) address space: every mapped page gets
    /// a fresh physical frame with identical content. Used by `fork`. No
    /// copy-on-write optimization -- acceptable for this workload's modest
    /// process count and memory footprint.
    ///
    /// Copies each page's *actual current* PTE flags from the parent,
    /// rather than re-deriving them from the owning `MapArea`'s nominal
    /// `perm`: `mmap(MAP_FIXED)` and `mprotect` both narrow/widen
    /// permissions on individual pages via direct PTE edits without
    /// updating the area's `perm` field (e.g. the dynamic linker
    /// temporarily marking part of a read-only segment writable to apply
    /// relocations). Rebuilding from the area's nominal `perm` would
    /// silently revert those per-page overrides in the child.
    pub fn from_existing(other: &MemorySet) -> Self {
        let mut memory_set = Self::new_bare();
        memory_set.map_trampoline();
        for area in other.areas.iter() {
            let mut new_area = MapArea::new(area.vpn_start.into(), area.vpn_end.into(), area.map_type, area.perm);
            let mut vpn = area.vpn_start;
            while vpn.0 < area.vpn_end.0 {
                let src_pte = other
                    .page_table
                    .translate(vpn)
                    .filter(|p| p.is_valid())
                    .unwrap_or_else(|| panic!("fork: area [{:#x},{:#x}) missing vpn {:#x}", area.vpn_start.0, area.vpn_end.0, vpn.0));
                let frame = frame_alloc().expect("out of memory during fork");
                let dst_ppn = frame.ppn;
                dst_ppn.as_bytes().copy_from_slice(src_pte.ppn().as_bytes());
                memory_set.page_table.map(vpn, dst_ppn, src_pte.flags());
                new_area.data_frames.insert(vpn, frame);
                vpn.0 += 1;
            }
            memory_set.areas.push(new_area);
        }
        memory_set.mmap_top = other.mmap_top;
        memory_set
    }

    pub fn token(&self) -> usize {
        self.page_table.token()
    }

    pub fn push(&mut self, mut area: MapArea, data: Option<&[u8]>) {
        area.map(&mut self.page_table);
        if let Some(data) = data {
            area.copy_data(&self.page_table, data);
        }
        self.areas.push(area);
    }

    pub fn insert_framed_area(&mut self, start_va: VirtAddr, end_va: VirtAddr, perm: MapPermission) {
        self.push(MapArea::new(start_va, end_va, MapType::Framed, perm), None);
    }

    pub fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some(idx) = self.areas.iter().position(|a| a.vpn_start == start_vpn) {
            let mut area = self.areas.remove(idx);
            area.unmap(&mut self.page_table);
        }
    }

    fn map_identical(&mut self, start: usize, end: usize, perm: MapPermission) {
        self.push(
            MapArea::new(VirtAddr(start), VirtAddr(end), MapType::Identical, perm),
            None,
        );
    }

    /// Kernel address space: identity-maps kernel code/data plus all free
    /// physical RAM plus MMIO regions, so kernel pointers keep working
    /// unchanged after paging is enabled.
    pub fn new_kernel() -> Self {
        unsafe extern "C" {
            fn stext();
            fn etext();
            fn srodata();
            fn erodata();
            fn sdata();
            fn edata();
            fn sbss();
            fn ebss();
            fn ekernel();
        }
        let mut memory_set = Self::new_bare();
        memory_set.map_identical(
            stext as usize,
            etext as usize,
            MapPermission::R | MapPermission::X,
        );
        memory_set.map_identical(srodata as usize, erodata as usize, MapPermission::R);
        memory_set.map_identical(
            sdata as usize,
            edata as usize,
            MapPermission::R | MapPermission::W,
        );
        memory_set.map_identical(
            sbss as usize,
            ebss as usize,
            MapPermission::R | MapPermission::W,
        );
        memory_set.map_identical(
            ekernel as usize,
            MEMORY_END,
            MapPermission::R | MapPermission::W,
        );
        for &(base, len) in MMIO {
            memory_set.map_identical(base, base + len, MapPermission::R | MapPermission::W);
        }
        memory_set
    }

    /// Activate this address space by writing `satp` and flushing the TLB.
    pub fn activate(&self) {
        let satp = self.token();
        unsafe {
            asm!("csrw satp, {}", "sfence.vma", in(reg) satp);
        }
    }
}

/// `xmas_elf` casts header/program-header structs directly out of the
/// input slice, which must therefore be 8-byte aligned; an embedded
/// `include_bytes!` array (or anything read off disk/tmpfs) makes no such
/// guarantee, so copy through an aligned buffer first.
fn align8(data: &[u8]) -> Vec<u8> {
    let words: Vec<u64> = data
        .chunks(8)
        .map(|c| {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            u64::from_ne_bytes(b)
        })
        .collect();
    unsafe { core::slice::from_raw_parts(words.as_ptr() as *const u8, data.len()) }.to_vec()
}

fn read_whole_file(path: &str) -> Option<Vec<u8>> {
    let file = crate::fs::open_file(path, 0)?;
    let size = file.size();
    let mut buf = alloc::vec![0u8; size];
    let mut off = 0;
    while off < size {
        let n = file.read_at(off, &mut buf[off..]);
        if n == 0 {
            break;
        }
        off += n;
    }
    Some(buf)
}

/// Map every `PT_LOAD` segment of `elf` at `p_vaddr + bias`, returning the
/// highest mapped `VirtPageNum` (one past the end).
fn load_segments(memory_set: &mut MemorySet, elf: &xmas_elf::ElfFile, bias: usize) -> VirtPageNum {
    let mut max_end_vpn = VirtPageNum(0);
    for ph in elf.program_iter() {
        if ph.get_type() != Ok(xmas_elf::program::Type::Load) {
            continue;
        }
        let start_va = VirtAddr(ph.virtual_addr() as usize + bias);
        let end_va = VirtAddr((ph.virtual_addr() + ph.mem_size()) as usize + bias);
        let mut perm = MapPermission::U;
        let flags = ph.flags();
        if flags.is_read() {
            perm |= MapPermission::R;
        }
        if flags.is_write() {
            perm |= MapPermission::W;
        }
        if flags.is_execute() {
            perm |= MapPermission::X;
        }
        let area = MapArea::new(start_va, end_va, MapType::Framed, perm);
        if area.vpn_end.0 > max_end_vpn.0 {
            max_end_vpn = area.vpn_end;
        }
        let data = &elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize];
        memory_set.push(area, Some(data));
    }
    max_end_vpn
}

const AT_NULL: usize = 0;
const AT_PHDR: usize = 3;
const AT_PHENT: usize = 4;
const AT_PHNUM: usize = 5;
const AT_PAGESZ: usize = 6;
const AT_BASE: usize = 7;
const AT_FLAGS: usize = 8;
const AT_ENTRY: usize = 9;
const AT_UID: usize = 11;
const AT_EUID: usize = 12;
const AT_GID: usize = 13;
const AT_EGID: usize = 14;
const AT_HWCAP: usize = 16;
const AT_CLKTCK: usize = 17;
const AT_SECURE: usize = 23;
const AT_RANDOM: usize = 25;
const AT_EXECFN: usize = 31;

/// Auxiliary vector entries whose value is fixed at build time; `AT_RANDOM`
/// and `AT_EXECFN` need pointers into the stack's string area, so
/// `build_init_stack` appends those two afterwards.
fn build_auxv(at_phdr: usize, at_phent: usize, at_phnum: usize, at_base: usize, at_entry: usize) -> Vec<(usize, usize)> {
    alloc::vec![
        (AT_PHDR, at_phdr),
        (AT_PHENT, at_phent),
        (AT_PHNUM, at_phnum),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, at_base),
        (AT_FLAGS, 0),
        (AT_ENTRY, at_entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_HWCAP, 0),
        (AT_CLKTCK, 100),
        (AT_SECURE, 0),
    ]
}

/// Lay out a Linux-ABI-compliant initial user stack (argv/envp/auxv and
/// their backing strings) at the top of the already-mapped stack area, and
/// return the resulting stack pointer.
fn build_init_stack(
    page_table: &PageTable,
    stack_top: usize,
    args: &[String],
    envs: &[String],
    auxv: &[(usize, usize)],
) -> usize {
    let mut rng_state = riscv::register::time::read64() ^ 0x9E3779B97F4A7C15;
    let mut next_rand = || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        rng_state
    };
    let random16: [u8; 16] = {
        let a = next_rand().to_ne_bytes();
        let b = next_rand().to_ne_bytes();
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&a);
        out[8..].copy_from_slice(&b);
        out
    };
    let execfn = args.first().cloned().unwrap_or_else(|| String::from(""));

    // Strings, packed back-to-back; record each one's offset within this
    // blob so absolute addresses can be computed once the blob's base is
    // known.
    let mut strings = Vec::new();
    let mut push_string = |blob: &mut Vec<u8>, s: &str| -> usize {
        let off = blob.len();
        blob.extend_from_slice(s.as_bytes());
        blob.push(0);
        off
    };
    let arg_offs: Vec<usize> = args.iter().map(|s| push_string(&mut strings, s)).collect();
    let env_offs: Vec<usize> = envs.iter().map(|s| push_string(&mut strings, s)).collect();
    let execfn_off = push_string(&mut strings, &execfn);
    let random_off = strings.len();
    strings.extend_from_slice(&random16);

    let mut auxv = auxv.to_vec();
    // Placeholders; patched to real addresses below once `strings_base` is known.
    auxv.push((AT_RANDOM, 0));
    auxv.push((AT_EXECFN, 0));
    auxv.push((AT_NULL, 0));

    let fixed_total = 8 // argc
        + 8 * (args.len() + 1)
        + 8 * (envs.len() + 1)
        + 16 * auxv.len();

    let strings_len = strings.len();
    let strings_end = stack_top;
    let strings_base = strings_end - strings_len;
    let raw_sp = strings_base - fixed_total;
    let final_sp = raw_sp & !15;
    let gap = strings_base - (final_sp + fixed_total);

    let random_va = strings_base + random_off;
    let execfn_va = strings_base + execfn_off;
    let n = auxv.len();
    auxv[n - 3] = (AT_RANDOM, random_va);
    auxv[n - 2] = (AT_EXECFN, execfn_va);

    let mut image = Vec::with_capacity(strings_end - final_sp);
    image.extend_from_slice(&(args.len() as u64).to_ne_bytes());
    for off in &arg_offs {
        image.extend_from_slice(&((strings_base + off) as u64).to_ne_bytes());
    }
    image.extend_from_slice(&0u64.to_ne_bytes());
    for off in &env_offs {
        image.extend_from_slice(&((strings_base + off) as u64).to_ne_bytes());
    }
    image.extend_from_slice(&0u64.to_ne_bytes());
    for (t, v) in &auxv {
        image.extend_from_slice(&(*t as u64).to_ne_bytes());
        image.extend_from_slice(&(*v as u64).to_ne_bytes());
    }
    image.resize(image.len() + gap, 0);
    image.extend_from_slice(&strings);

    debug_assert_eq!(final_sp + image.len(), strings_end);
    write_user_bytes(page_table, final_sp, &image);
    final_sp
}

/// Copy `data` into user memory starting at virtual address `va`, crossing
/// page boundaries as needed.
fn write_user_bytes(page_table: &PageTable, va: usize, data: &[u8]) {
    let mut remaining = data;
    let mut cur_va = va;
    while !remaining.is_empty() {
        let pa = page_table
            .translate_va(VirtAddr(cur_va))
            .unwrap_or_else(|| panic!("unmapped stack address {:#x}", cur_va));
        let page_off = pa.page_offset();
        let n = remaining.len().min(PAGE_SIZE - page_off);
        let dst = unsafe { core::slice::from_raw_parts_mut(pa.0 as *mut u8, n) };
        dst.copy_from_slice(&remaining[..n]);
        remaining = &remaining[n..];
        cur_va += n;
    }
}
