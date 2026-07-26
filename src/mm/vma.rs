//! Address spaces and virtual memory areas.
//!
//! Each user process owns an `AddrSpace`: a page table plus a sorted list of
//! VMAs describing what each region means. Pages are populated lazily on fault,
//! and `fork` shares them copy-on-write.

use super::addr::*;
use super::frame;
use super::page_table::{activate, flush_tlb_all, flush_tlb_page, PTEFlags, PageTable};
use crate::fs::File;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

bitflags! {
    /// Protection bits for a VMA, matching Linux `PROT_*`.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Prot: u32 {
        const READ = 1;
        const WRITE = 2;
        const EXEC = 4;
    }
}

impl Prot {
    /// The PTE flags a fully-populated, non-COW page in this VMA should carry.
    fn pte_flags(self) -> PTEFlags {
        let mut f = PTEFlags::V | PTEFlags::U | PTEFlags::A;
        // Linux treats PROT_WRITE and PROT_EXEC as implying readability, and
        // musl maps some regions write-only; RISC-V requires R for W to be
        // meaningful, so always set R.
        f |= PTEFlags::R;
        if self.contains(Prot::WRITE) {
            f |= PTEFlags::W | PTEFlags::D;
        }
        if self.contains(Prot::EXEC) {
            f |= PTEFlags::X;
        }
        f
    }
}

/// What backs the pages of a VMA.
#[derive(Clone)]
pub enum Backing {
    /// Zero-filled anonymous memory.
    Anon,
    /// Contents read from a file at `offset + (va - start)`.
    File {
        file: Arc<File>,
        offset: usize,
        /// Hard limit: never read at or past this file offset; the remainder of
        /// the page reads as zero.
        ///
        /// ELF segments are not page aligned, so the last page of a segment's
        /// file-backed part usually extends past `p_filesz` into the `.bss`.
        /// Reading the file that far would splice whatever section follows in the
        /// image (typically `.riscv.attributes`) into the program's zeroed data.
        limit: usize,
    },
}

/// A virtual memory area.
#[derive(Clone)]
pub struct Vma {
    pub start: usize,
    pub end: usize,
    pub prot: Prot,
    pub backing: Backing,
    /// MAP_SHARED: writes go to the underlying file/frames rather than a private
    /// copy. We implement shared anonymous memory (used for nginx's shared zones)
    /// by keeping the frames shared instead of marking them COW on fork.
    pub shared: bool,
    /// A name for debugging (`[stack]`, `[heap]`, the mapped file, ...).
    pub name: &'static str,
}

impl Vma {
    pub fn contains(&self, va: usize) -> bool {
        va >= self.start && va < self.end
    }
}

pub struct AddrSpace {
    pub page_table: PageTable,
    /// VMAs keyed by start address.
    pub areas: BTreeMap<usize, Vma>,
    /// Program break for `brk`.
    pub brk: usize,
    pub brk_start: usize,
    /// Next address to hand out for an anonymous `mmap`.
    mmap_next: usize,
}

impl AddrSpace {
    pub fn new() -> Option<Self> {
        let mut page_table = PageTable::new()?;
        page_table.clone_kernel_mappings(kernel_page_table());
        Some(Self {
            page_table,
            areas: BTreeMap::new(),
            brk: 0,
            brk_start: 0,
            mmap_next: USER_MMAP_BASE,
        })
    }

    pub fn satp(&self) -> usize {
        self.page_table.satp()
    }

    /// Find the VMA containing `va`.
    pub fn find_vma(&self, va: usize) -> Option<&Vma> {
        self.areas
            .range(..=va)
            .next_back()
            .map(|(_, v)| v)
            .filter(|v| v.contains(va))
    }

    /// Insert a VMA, assuming the range is already clear.
    pub fn insert_vma(&mut self, vma: Vma) {
        self.areas.insert(vma.start, vma);
    }

    /// Add a mapping, removing anything that overlaps first (MAP_FIXED
    /// semantics, which `ld.so` relies on when it maps segments over its
    /// initial reservation).
    pub fn map_region(
        &mut self,
        start: usize,
        end: usize,
        prot: Prot,
        backing: Backing,
        shared: bool,
        name: &'static str,
    ) {
        let start = page_down(start);
        let end = page_up(end);
        if start >= end {
            return;
        }
        self.unmap_range(start, end);
        self.insert_vma(Vma {
            start,
            end,
            prot,
            backing,
            shared,
            name,
        });
    }

    /// Grow or shrink the VMA starting exactly at `start` to end at `new_end`,
    /// without disturbing the pages already populated inside it.
    ///
    /// This is what `brk` and an in-place `mremap` need: `map_region` would
    /// unmap the old range first and throw away the program's live heap.
    ///
    /// Returns false if no VMA starts at `start`.
    pub fn resize_vma(&mut self, start: usize, new_end: usize) -> bool {
        let new_end = page_up(new_end);
        let Some(vma) = self.areas.get_mut(&start) else {
            return false;
        };
        let old_end = vma.end;
        if new_end == old_end {
            return true;
        }
        // Don't let a resize swallow a neighbour.
        if new_end > old_end {
            let blocked = self
                .areas
                .range(old_end..new_end)
                .next()
                .map(|(&k, _)| k)
                .is_some();
            if blocked {
                return false;
            }
        }
        self.areas.get_mut(&start).unwrap().end = new_end;
        // Shrinking releases the pages above the new end.
        if new_end < old_end {
            let mut va = new_end;
            while va < old_end {
                if let Some(pa) = self.page_table.unmap(va) {
                    frame::decref(pa);
                }
                va += PAGE_SIZE;
            }
            flush_tlb_all();
        }
        true
    }

    /// Remove all mappings in `[start, end)`, splitting VMAs as needed.
    pub fn unmap_range(&mut self, start: usize, end: usize) {
        let start = page_down(start);
        let end = page_up(end);
        if start >= end {
            return;
        }

        // Collect overlapping VMAs. `range(..end)` catches every VMA starting
        // before `end`; we then filter on the end bound.
        let overlapping: Vec<usize> = self
            .areas
            .range(..end)
            .filter(|(_, v)| v.end > start)
            .map(|(&k, _)| k)
            .collect();

        for key in overlapping {
            let vma = self.areas.remove(&key).unwrap();
            // Keep the part before the hole.
            if vma.start < start {
                let mut left = vma.clone();
                left.end = start;
                self.areas.insert(left.start, left);
            }
            // Keep the part after the hole, adjusting the file offset.
            if vma.end > end {
                let mut right = vma.clone();
                right.start = end;
                if let Backing::File { offset, .. } = &mut right.backing {
                    *offset += end - vma.start;
                }
                self.areas.insert(right.start, right);
            }
        }

        // Tear down the page table entries and release the frames.
        // (`limit` is an absolute file offset, so splitting leaves it unchanged.)
        let mut va = start;
        while va < end {
            if let Some(pa) = self.page_table.unmap(va) {
                frame::decref(pa);
            }
            va += PAGE_SIZE;
        }
        flush_tlb_all();
    }

    /// Change protection on a range. Splits VMAs at the boundaries.
    pub fn protect_range(&mut self, start: usize, end: usize, prot: Prot) {
        let start = page_down(start);
        let end = page_up(end);
        if start >= end {
            return;
        }

        let overlapping: Vec<usize> = self
            .areas
            .range(..end)
            .filter(|(_, v)| v.end > start)
            .map(|(&k, _)| k)
            .collect();

        for key in overlapping {
            let vma = self.areas.remove(&key).unwrap();
            // Piece before the range keeps its old prot.
            if vma.start < start {
                let mut left = vma.clone();
                left.end = start;
                self.areas.insert(left.start, left);
            }
            // Piece after the range keeps its old prot.
            if vma.end > end {
                let mut right = vma.clone();
                right.start = end;
                if let Backing::File { offset, .. } = &mut right.backing {
                    *offset += end - vma.start;
                }
                self.areas.insert(right.start, right);
            }
            // The overlapping middle gets the new prot.
            let mid_start = vma.start.max(start);
            let mid_end = vma.end.min(end);
            let mut mid = vma.clone();
            mid.start = mid_start;
            mid.end = mid_end;
            mid.prot = prot;
            if let Backing::File { offset, .. } = &mut mid.backing {
                *offset += mid_start - vma.start;
            }
            self.areas.insert(mid.start, mid);
        }

        // Update any already-present PTEs to the new permissions. Pages that
        // were COW must stay read-only so the fault handler still copies them,
        // but we record the new logical writability in SOFT_W.
        let mut va = start;
        while va < end {
            if let Some(pte) = self.page_table.lookup_mut(va) {
                let was_cow = pte.is_cow();
                let mut flags = prot.pte_flags();
                if was_cow {
                    flags.remove(PTEFlags::W);
                    flags.remove(PTEFlags::D);
                    flags |= PTEFlags::COW;
                    if prot.contains(Prot::WRITE) {
                        flags |= PTEFlags::SOFT_W;
                    }
                }
                pte.set_flags(flags);
            }
            va += PAGE_SIZE;
        }
        flush_tlb_all();
    }

    /// Find a free range of `len` bytes for an anonymous mmap.
    pub fn find_free_area(&mut self, len: usize) -> Option<usize> {
        let len = page_up(len);
        // Try the bump pointer first; it is the common case and keeps the
        // search O(1) for the typical monotonically-growing allocation pattern.
        let mut candidate = self.mmap_next;
        loop {
            if candidate + len > USER_MMAP_TOP {
                // Wrap around and do a full scan of the gaps.
                return self.scan_for_gap(len);
            }
            // Does anything overlap [candidate, candidate + len)?
            let conflict = self
                .areas
                .range(..candidate + len)
                .filter(|(_, v)| v.end > candidate)
                .map(|(_, v)| v.end)
                .max();
            match conflict {
                Some(end) => candidate = page_up(end),
                None => {
                    self.mmap_next = candidate + len;
                    return Some(candidate);
                }
            }
        }
    }

    fn scan_for_gap(&mut self, len: usize) -> Option<usize> {
        let mut cursor = USER_MMAP_BASE;
        for (_, vma) in self.areas.range(USER_MMAP_BASE..USER_MMAP_TOP) {
            if vma.start >= cursor + len {
                self.mmap_next = cursor + len;
                return Some(cursor);
            }
            cursor = cursor.max(page_up(vma.end));
        }
        if cursor + len <= USER_MMAP_TOP {
            self.mmap_next = cursor + len;
            Some(cursor)
        } else {
            None
        }
    }

    /// Ensure the page containing `va` is present and satisfies `write` access.
    /// Returns false if the access is illegal (segfault).
    pub fn handle_fault(&mut self, va: usize, write: bool, exec: bool) -> bool {
        let va = page_down(va);
        let Some(vma) = self.find_vma(va).cloned() else {
            return false;
        };

        // Permission check against the VMA's declared protection.
        if write && !vma.prot.contains(Prot::WRITE) {
            return false;
        }
        if exec && !vma.prot.contains(Prot::EXEC) {
            return false;
        }
        if !write && !exec && !vma.prot.intersects(Prot::READ | Prot::WRITE | Prot::EXEC) {
            return false;
        }

        // Already mapped? Then this is either a COW fault or a spurious fault.
        if let Some(pte) = self.page_table.lookup(va) {
            if pte.is_cow() && write {
                return self.do_cow(va, &vma);
            }
            // Present with sufficient permissions: another hart or an earlier
            // fault already fixed it up. Flush and retry.
            flush_tlb_page(va);
            return true;
        }

        // Populate a fresh page.
        let Some(pa) = frame::alloc_frame() else {
            return false;
        };
        if let Backing::File {
            file,
            offset,
            limit,
        } = &vma.backing
        {
            let file_off = offset + (va - vma.start);
            if file_off < *limit {
                // Read no further than the limit, so bytes belonging to the
                // segment's zero-filled tail stay zero.
                let want = (*limit - file_off).min(PAGE_SIZE);
                let buf = unsafe { phys_slice(pa, want) };
                let _ = file.read_at(file_off, buf);
            }
        }
        self.page_table.map(va, pa, vma.prot.pte_flags());
        flush_tlb_page(va);
        true
    }

    /// Resolve a copy-on-write fault by giving this address space a private copy.
    fn do_cow(&mut self, va: usize, vma: &Vma) -> bool {
        // Only copy if the VMA is logically writable.
        if !vma.prot.contains(Prot::WRITE) {
            return false;
        }
        let pte = self.page_table.lookup_mut(va).unwrap();
        let old_pa = pte.phys_addr();
        if frame::refcount(old_pa) == 1 {
            // Sole owner: just restore write permission in place.
            let mut flags = vma.prot.pte_flags();
            flags.remove(PTEFlags::COW);
            flags.remove(PTEFlags::SOFT_W);
            pte.set_flags(flags);
            flush_tlb_page(va);
            return true;
        }
        let Some(new_pa) = frame::alloc_frame_dirty() else {
            return false;
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                phys_to_virt(old_pa) as *const u8,
                phys_to_virt(new_pa) as *mut u8,
                PAGE_SIZE,
            );
        }
        let mut flags = vma.prot.pte_flags();
        flags.remove(PTEFlags::COW);
        flags.remove(PTEFlags::SOFT_W);
        self.page_table.map(va, new_pa, flags);
        frame::decref(old_pa);
        flush_tlb_page(va);
        true
    }

    /// Force a range to be present, e.g. before the kernel writes to it.
    pub fn populate(&mut self, start: usize, end: usize, write: bool) -> bool {
        let mut va = page_down(start);
        while va < end {
            if !self.handle_fault(va, write, false) {
                return false;
            }
            va += PAGE_SIZE;
        }
        true
    }

    /// Duplicate this address space for `fork`, sharing pages copy-on-write.
    pub fn fork(&mut self) -> Option<Self> {
        let mut child = Self::new()?;
        child.brk = self.brk;
        child.brk_start = self.brk_start;
        child.mmap_next = self.mmap_next;
        child.areas = self.areas.clone();

        // Copy each present leaf mapping. Shared mappings stay shared; private
        // writable ones become COW in both address spaces.
        let mut updates: Vec<(usize, usize, PTEFlags)> = Vec::new();
        let areas = &self.areas;
        self.page_table.for_each_user_leaf(|va, pte| {
            let pa = pte.phys_addr();
            let shared = areas
                .range(..=va)
                .next_back()
                .map(|(_, v)| v.contains(va) && v.shared)
                .unwrap_or(false);

            let flags = if shared {
                // Keep both sides pointing at the same writable frame.
                pte.flags()
            } else if pte.is_writable() || pte.is_cow() {
                let mut f = pte.flags();
                f.remove(PTEFlags::W);
                f.remove(PTEFlags::D);
                f |= PTEFlags::COW | PTEFlags::SOFT_W;
                // Demote the parent too, so its writes also trigger a copy.
                pte.set_flags(f);
                f
            } else {
                pte.flags()
            };
            frame::incref(pa);
            updates.push((va, pa, flags));
        });

        for (va, pa, flags) in updates {
            child.page_table.map(va, pa, flags)?;
        }
        flush_tlb_all();
        Some(child)
    }

    /// Release every user mapping (used by `execve` before loading a new image).
    pub fn clear_user(&mut self) {
        let mut pages: Vec<usize> = Vec::new();
        self.page_table.for_each_user_leaf(|va, _| pages.push(va));
        for va in pages {
            if let Some(pa) = self.page_table.unmap(va) {
                frame::decref(pa);
            }
        }
        self.areas.clear();
        self.mmap_next = USER_MMAP_BASE;
        self.brk = 0;
        self.brk_start = 0;
        flush_tlb_all();
    }

    pub fn activate(&self) {
        activate(self.satp());
    }
}

impl Drop for AddrSpace {
    fn drop(&mut self) {
        self.clear_user();
    }
}

// ---------------------------------------------------------------------------
// The kernel's own page table
// ---------------------------------------------------------------------------

static mut KERNEL_PAGE_TABLE: Option<PageTable> = None;

pub fn kernel_page_table() -> &'static PageTable {
    unsafe {
        #[allow(static_mut_refs)]
        KERNEL_PAGE_TABLE.as_ref().expect("kernel page table not built")
    }
}

/// Build the kernel page table: identity-map RAM and MMIO with gigapages, then
/// enable paging.
pub fn init_kernel_page_table() {
    let mut pt = PageTable::new().expect("no memory for kernel page table");

    // Identity map the low 4 GiB with 1 GiB pages. This covers:
    //   0x0000_0000 - 0x4000_0000 : MMIO (CLINT, PLIC, UART, virtio-mmio, ...)
    //   0x8000_0000 - 0xC000_0000 : RAM
    for gb in 0..4usize {
        let addr = gb << 30;
        pt.map_gigapage(addr, addr, PTEFlags::KERNEL_RWX | PTEFlags::G);
    }

    let satp = pt.satp();
    unsafe {
        KERNEL_PAGE_TABLE = Some(pt);
    }
    activate(satp);
    crate::info!("paging enabled (Sv39, satp={:#x})", satp);
}
