//! User address spaces: VMAs, demand paging, copy-on-write fork.
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

use super::frame::{Frame, SharedFrame};
use super::page_table::{flush_tlb, flush_tlb_page, PageTable, PteFlags};
use crate::config::*;
use crate::fs::file::File;

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Prot: u32 {
        const R = 1;
        const W = 2;
        const X = 4;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccessKind {
    Read,
    Write,
    Exec,
}

#[derive(Clone)]
pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub prot: Prot,
    pub shared: bool,
    /// Backing file and the file offset corresponding to `start`.
    pub file: Option<(Arc<File>, u64)>,
    pub grows_down: bool,
}

impl Vma {
    fn contains(&self, va: usize) -> bool {
        va >= self.start && va < self.end
    }
}

pub struct AddressSpace {
    pub pt: PageTable,
    vmas: BTreeMap<usize, Vma>,
    pages: BTreeMap<usize, SharedFrame>,
    pub brk_start: usize,
    pub brk: usize,
    mmap_hint: usize,
}

#[derive(Debug)]
pub enum FaultError {
    NoMapping,
    Protection,
    Io,
}

fn page_down(x: usize) -> usize {
    x & !(PAGE_SIZE - 1)
}
pub fn page_up(x: usize) -> usize {
    (x + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

impl AddressSpace {
    pub fn new() -> Self {
        AddressSpace {
            pt: PageTable::new_kernel(),
            vmas: BTreeMap::new(),
            pages: BTreeMap::new(),
            brk_start: 0,
            brk: 0,
            mmap_hint: MMAP_BASE,
        }
    }

    pub fn satp(&self) -> usize {
        self.pt.satp()
    }

    pub fn find_vma(&self, va: usize) -> Option<&Vma> {
        let (_, v) = self.vmas.range(..=va).next_back()?;
        if v.contains(va) {
            Some(v)
        } else {
            None
        }
    }

    fn find_vma_mut(&mut self, va: usize) -> Option<&mut Vma> {
        let (_, v) = self.vmas.range_mut(..=va).next_back()?;
        if v.contains(va) {
            Some(v)
        } else {
            None
        }
    }

    pub fn vmas(&self) -> impl Iterator<Item = &Vma> {
        self.vmas.values()
    }

    /// Is [start, end) completely free?
    pub fn is_free(&self, start: usize, end: usize) -> bool {
        if end > USER_MAX || start >= end {
            return false;
        }
        // any vma overlapping?
        if let Some((_, v)) = self.vmas.range(..end).next_back() {
            if v.end > start {
                return false;
            }
        }
        true
    }

    /// Find a free region of `len` bytes at or above `MMAP_BASE`.
    pub fn find_free(&mut self, len: usize, hint: usize) -> Option<usize> {
        let len = page_up(len);
        if hint != 0
            && hint % PAGE_SIZE == 0
            && self.is_free(hint, hint + len)
            && hint + len <= USER_STACK_TOP - USER_STACK_SIZE
        {
            return Some(hint);
        }
        let mut cand = self.mmap_hint;
        loop {
            if cand + len > USER_STACK_TOP - USER_STACK_SIZE - PAGE_SIZE {
                // wrap once
                if cand == MMAP_BASE {
                    return None;
                }
                cand = MMAP_BASE;
                self.mmap_hint = MMAP_BASE;
                continue;
            }
            match self.vmas.range(..cand + len).next_back() {
                Some((_, v)) if v.end > cand => {
                    cand = page_up(v.end);
                }
                _ => {
                    self.mmap_hint = cand + len;
                    return Some(cand);
                }
            }
        }
    }

    /// Insert a VMA; the range must be free.
    pub fn insert_vma(&mut self, vma: Vma) {
        debug_assert!(vma.start < vma.end);
        debug_assert!(self.is_free(vma.start, vma.end), "vma range not free: {:#x}-{:#x}", vma.start, vma.end);
        self.vmas.insert(vma.start, vma);
    }

    /// Map an anonymous or file region. Returns the start address.
    pub fn mmap(
        &mut self,
        addr: usize,
        len: usize,
        prot: Prot,
        shared: bool,
        fixed: bool,
        file: Option<(Arc<File>, u64)>,
    ) -> Result<usize, i32> {
        let len = page_up(len);
        if len == 0 {
            return Err(crate::abi::EINVAL);
        }
        let start = if fixed {
            if addr % PAGE_SIZE != 0 || addr + len > USER_MAX {
                return Err(crate::abi::EINVAL);
            }
            self.munmap(addr, len);
            addr
        } else {
            self.find_free(len, addr).ok_or(crate::abi::ENOMEM)?
        };
        self.insert_vma(Vma { start, end: start + len, prot, shared, file, grows_down: false });
        Ok(start)
    }

    /// Split the VMA containing `at` so that `at` becomes a boundary.
    fn split_at(&mut self, at: usize) {
        let Some(v) = self.find_vma_mut(at) else { return };
        if v.start == at {
            return;
        }
        let mut right = v.clone();
        let old_end = v.end;
        let vstart = v.start;
        v.end = at;
        right.start = at;
        right.end = old_end;
        if let Some((_, off)) = &mut right.file {
            *off += (at - vstart) as u64;
        }
        self.vmas.insert(at, right);
    }

    fn unmap_pages(&mut self, start: usize, end: usize) {
        let keys: Vec<usize> = self.pages.range(start..end).map(|(k, _)| *k).collect();
        for va in keys {
            self.pages.remove(&va);
            self.pt.unmap(va);
        }
        if end - start <= 16 * PAGE_SIZE {
            let mut va = start;
            while va < end {
                flush_tlb_page(va);
                va += PAGE_SIZE;
            }
        } else {
            flush_tlb();
        }
    }

    pub fn munmap(&mut self, addr: usize, len: usize) {
        let start = page_down(addr);
        let end = page_up(addr + len);
        if start >= end {
            return;
        }
        self.split_at(start);
        self.split_at(end);
        let keys: Vec<usize> = self.vmas.range(start..end).map(|(k, _)| *k).collect();
        for k in keys {
            self.vmas.remove(&k);
        }
        self.unmap_pages(start, end);
    }

    pub fn mprotect(&mut self, addr: usize, len: usize, prot: Prot) -> Result<(), i32> {
        let start = page_down(addr);
        let end = page_up(addr + len);
        if start >= end || end > USER_MAX {
            return Err(crate::abi::EINVAL);
        }
        self.split_at(start);
        self.split_at(end);
        let keys: Vec<usize> = self.vmas.range(start..end).map(|(k, _)| *k).collect();
        for k in keys {
            let v = self.vmas.get_mut(&k).unwrap();
            v.prot = prot;
            let (vs, ve, shared) = (v.start, v.end, v.shared);
            let pages: Vec<(usize, usize)> =
                self.pages.range(vs..ve).map(|(k, f)| (*k, Arc::strong_count(f))).collect();
            for (va, rc) in pages {
                let mut flags = PteFlags::U;
                if prot.contains(Prot::R) {
                    flags |= PteFlags::R;
                }
                if prot.contains(Prot::X) {
                    flags |= PteFlags::X;
                }
                if prot.contains(Prot::W) && (shared || rc == 1) {
                    flags |= PteFlags::W;
                }
                self.pt.set_flags(va, flags);
            }
        }
        flush_tlb();
        Ok(())
    }

    fn pte_flags_for(prot: Prot, writable_now: bool) -> PteFlags {
        let mut flags = PteFlags::U;
        if prot.contains(Prot::R) {
            flags |= PteFlags::R;
        }
        if prot.contains(Prot::X) {
            flags |= PteFlags::X;
        }
        if prot.contains(Prot::W) && writable_now {
            flags |= PteFlags::W;
        }
        flags
    }

    /// Handle a page fault at `va`. Returns Ok if the access can now be retried.
    pub fn handle_fault(&mut self, va: usize, kind: AccessKind) -> Result<(), FaultError> {
        if va >= USER_MAX {
            return Err(FaultError::NoMapping);
        }
        let page = page_down(va);
        let vma = self.find_vma(va).ok_or(FaultError::NoMapping)?.clone();
        let ok = match kind {
            AccessKind::Read => vma.prot.contains(Prot::R) || vma.prot.contains(Prot::X) || vma.prot.contains(Prot::W),
            AccessKind::Write => vma.prot.contains(Prot::W),
            AccessKind::Exec => vma.prot.contains(Prot::X),
        };
        if !ok {
            return Err(FaultError::Protection);
        }

        if let Some(frame) = self.pages.get(&page).cloned() {
            // present: must be a CoW write (or a spurious fault)
            if kind == AccessKind::Write {
                if vma.shared || Arc::strong_count(&frame) == 1 {
                    self.pt.set_flags(page, Self::pte_flags_for(vma.prot, true));
                } else {
                    let new = Frame::alloc();
                    new.copy_from(&frame);
                    let new = Arc::new(new);
                    self.pt.map(page, new.pa(), Self::pte_flags_for(vma.prot, true));
                    self.pages.insert(page, new);
                }
                flush_tlb_page(page);
                return Ok(());
            }
            // read fault on a present page: make sure PTE has proper perms
            let cur = self.pt.get(page).map(|p| p.flags()).unwrap_or(PteFlags::empty());
            let want = Self::pte_flags_for(vma.prot, false);
            if !cur.contains(want) {
                let rc = Arc::strong_count(&frame);
                self.pt.set_flags(page, Self::pte_flags_for(vma.prot, vma.shared || rc == 1) | (cur & PteFlags::W));
            }
            flush_tlb_page(page);
            return Ok(());
        }

        // not present: allocate & fill
        let frame = Frame::alloc();
        if let Some((file, off)) = &vma.file {
            let foff = *off + (page - vma.start) as u64;
            let buf = frame.as_mut_slice();
            let mut done = 0;
            while done < PAGE_SIZE {
                match file.pread(&mut buf[done..], foff + done as u64) {
                    Ok(0) => break,
                    Ok(n) => done += n,
                    Err(_) => return Err(FaultError::Io),
                }
            }
        }
        let writable = vma.prot.contains(Prot::W);
        self.pt.map(page, frame.pa(), Self::pte_flags_for(vma.prot, writable));
        self.pages.insert(page, Arc::new(frame));
        flush_tlb_page(page);
        Ok(())
    }

    /// Ensure `va` is accessible for `kind` (faulting in if needed) and return the PA.
    pub fn access(&mut self, va: usize, kind: AccessKind) -> Option<usize> {
        if va >= USER_MAX {
            return None;
        }
        if let Some((pa, flags)) = self.pt.translate(va) {
            let ok = match kind {
                AccessKind::Write => flags.contains(PteFlags::W),
                AccessKind::Read => flags.contains(PteFlags::R),
                AccessKind::Exec => flags.contains(PteFlags::X),
            };
            if ok && flags.contains(PteFlags::U) {
                return Some(pa);
            }
        }
        self.handle_fault(va, kind).ok()?;
        self.pt.translate(va).map(|(pa, _)| pa)
    }

    /// Fork: copy-on-write duplicate.
    pub fn fork(&mut self) -> AddressSpace {
        let mut child = AddressSpace::new();
        child.vmas = self.vmas.clone();
        child.brk_start = self.brk_start;
        child.brk = self.brk;
        child.mmap_hint = self.mmap_hint;
        let entries: Vec<(usize, SharedFrame)> = self.pages.iter().map(|(k, v)| (*k, v.clone())).collect();
        for (va, frame) in entries {
            let vma = self.find_vma(va).expect("page without vma").clone();
            if vma.shared {
                let flags = Self::pte_flags_for(vma.prot, true);
                child.pt.map(va, frame.pa(), flags);
            } else {
                let flags = Self::pte_flags_for(vma.prot, false);
                self.pt.set_flags(va, flags);
                child.pt.map(va, frame.pa(), flags);
            }
            child.pages.insert(va, frame);
        }
        flush_tlb();
        child
    }

    pub fn set_brk(&mut self, new_brk: usize) -> usize {
        if self.brk_start == 0 {
            return self.brk;
        }
        let new_brk = new_brk.max(self.brk_start);
        let cur_end = page_up(self.brk);
        let new_end = page_up(new_brk);
        if new_end > cur_end {
            if !self.is_free(cur_end, new_end) {
                return self.brk;
            }
            // Extend the anonymous vma ending exactly at cur_end, else insert a new one.
            let extend = match self.find_vma(cur_end - 1) {
                Some(v)
                    if v.end == cur_end
                        && v.file.is_none()
                        && !v.shared
                        && v.prot == (Prot::R | Prot::W)
                        && cur_end > self.brk_start =>
                {
                    Some(v.start)
                }
                _ => None,
            };
            match extend {
                Some(k) => self.vmas.get_mut(&k).unwrap().end = new_end,
                None => self.insert_vma(Vma {
                    start: cur_end,
                    end: new_end,
                    prot: Prot::R | Prot::W,
                    shared: false,
                    file: None,
                    grows_down: false,
                }),
            }
        } else if new_end < cur_end {
            self.munmap(new_end, cur_end - new_end);
        }
        self.brk = new_brk;
        self.brk
    }

    /// Grow the VMA starting at `start` to `new_end` if the space is free.
    pub fn extend_vma(&mut self, start: usize, new_end: usize) -> bool {
        let Some(v) = self.vmas.get(&start) else { return false };
        let old_end = v.end;
        if new_end <= old_end {
            return true;
        }
        if !self.is_free(old_end, new_end) {
            return false;
        }
        self.vmas.get_mut(&start).unwrap().end = new_end;
        true
    }

    pub fn resident_pages(&self) -> usize {
        self.pages.len()
    }

    pub fn dump(&self) {
        for v in self.vmas.values() {
            crate::println!(
                "  {:#014x}-{:#014x} {}{}{} {} {}",
                v.start,
                v.end,
                if v.prot.contains(Prot::R) { 'r' } else { '-' },
                if v.prot.contains(Prot::W) { 'w' } else { '-' },
                if v.prot.contains(Prot::X) { 'x' } else { '-' },
                if v.shared { 's' } else { 'p' },
                match &v.file {
                    Some((f, off)) => alloc::format!("{}+{:#x}", f.path(), off),
                    None => alloc::string::String::from("[anon]"),
                }
            );
        }
    }
}
