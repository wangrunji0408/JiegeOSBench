//! Sv39 page tables.

use super::addr::*;
use super::frame;
use alloc::vec::Vec;
use bitflags::bitflags;

bitflags! {
    /// Sv39 page table entry flags.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct PTEFlags: usize {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
        /// Software bit: this mapping is copy-on-write.
        const COW = 1 << 8;
        /// Software bit: the VMA is logically writable (used with COW to know
        /// whether a write fault should be resolved by copying).
        const SOFT_W = 1 << 9;
    }
}

/// Convenience flag sets.
impl PTEFlags {
    pub const KERNEL_RW: Self = Self::from_bits_truncate(
        Self::V.bits() | Self::R.bits() | Self::W.bits() | Self::A.bits() | Self::D.bits(),
    );
    pub const KERNEL_RX: Self = Self::from_bits_truncate(
        Self::V.bits() | Self::R.bits() | Self::X.bits() | Self::A.bits(),
    );
    pub const KERNEL_RWX: Self = Self::from_bits_truncate(
        Self::V.bits()
            | Self::R.bits()
            | Self::W.bits()
            | Self::X.bits()
            | Self::A.bits()
            | Self::D.bits(),
    );
}

const PPN_MASK: usize = ((1usize << 44) - 1) << 10;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(pub usize);

impl PageTableEntry {
    #[inline]
    pub fn new(pa: usize, flags: PTEFlags) -> Self {
        Self(((pa >> PAGE_SHIFT) << 10) | flags.bits())
    }

    #[inline]
    pub fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub fn phys_addr(&self) -> usize {
        ((self.0 & PPN_MASK) >> 10) << PAGE_SHIFT
    }

    #[inline]
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0)
    }

    #[inline]
    pub fn set_flags(&mut self, flags: PTEFlags) {
        self.0 = (self.0 & PPN_MASK) | flags.bits();
    }

    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & PTEFlags::V.bits() != 0
    }

    /// A leaf entry maps memory; a non-leaf points at the next level table.
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.is_valid() && self.0 & (PTEFlags::R.bits() | PTEFlags::X.bits() | PTEFlags::W.bits()) != 0
    }

    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & PTEFlags::W.bits() != 0
    }

    #[inline]
    pub fn is_cow(&self) -> bool {
        self.0 & PTEFlags::COW.bits() != 0
    }
}

/// Extract the three 9-bit VPN indices from a virtual address.
#[inline]
fn vpn_indices(va: usize) -> [usize; 3] {
    let vpn = va >> PAGE_SHIFT;
    [(vpn >> 18) & 0x1ff, (vpn >> 9) & 0x1ff, vpn & 0x1ff]
}

/// An Sv39 page table. Owns its intermediate tables (but not the leaf frames,
/// which are owned by the address space's VMAs / frame refcounts).
pub struct PageTable {
    root: usize,
    /// Intermediate tables allocated by this page table, freed on drop.
    intermediate: Vec<usize>,
}

impl PageTable {
    pub fn new() -> Option<Self> {
        let root = frame::alloc_frame()?;
        Some(Self {
            root,
            intermediate: Vec::new(),
        })
    }

    #[inline]
    pub fn root_paddr(&self) -> usize {
        self.root
    }

    /// The value to write into `satp` to activate this table.
    #[inline]
    pub fn satp(&self) -> usize {
        // mode = 8 (Sv39), ASID = 0
        (8usize << 60) | (self.root >> PAGE_SHIFT)
    }

    #[inline]
    fn table_at(pa: usize) -> &'static mut [PageTableEntry; 512] {
        unsafe { &mut *(phys_to_virt(pa) as *mut [PageTableEntry; 512]) }
    }

    /// Walk to the leaf PTE for `va`, allocating intermediate tables as needed.
    fn walk_create(&mut self, va: usize) -> Option<&'static mut PageTableEntry> {
        let idx = vpn_indices(va);
        let mut table_pa = self.root;
        for level in 0..2 {
            let entry = &mut Self::table_at(table_pa)[idx[level]];
            if !entry.is_valid() {
                let next = frame::alloc_frame()?;
                self.intermediate.push(next);
                *entry = PageTableEntry::new(next, PTEFlags::V);
                table_pa = next;
            } else {
                table_pa = entry.phys_addr();
            }
        }
        Some(&mut Self::table_at(table_pa)[idx[2]])
    }

    /// Walk to the leaf PTE for `va` without allocating.
    fn walk(&self, va: usize) -> Option<&'static mut PageTableEntry> {
        let idx = vpn_indices(va);
        let mut table_pa = self.root;
        for level in 0..2 {
            let entry = &Self::table_at(table_pa)[idx[level]];
            if !entry.is_valid() {
                return None;
            }
            if entry.is_leaf() {
                // Huge page: we don't split these, and only the kernel identity
                // region uses them, so report absence for lookups.
                return None;
            }
            table_pa = entry.phys_addr();
        }
        Some(&mut Self::table_at(table_pa)[idx[2]])
    }

    /// Map a single 4 KiB page. Overwrites any existing mapping.
    pub fn map(&mut self, va: usize, pa: usize, flags: PTEFlags) -> Option<()> {
        let pte = self.walk_create(va)?;
        *pte = PageTableEntry::new(pa, flags | PTEFlags::V);
        Some(())
    }

    /// Remove a mapping, returning the physical address it pointed at.
    pub fn unmap(&mut self, va: usize) -> Option<usize> {
        let pte = self.walk(va)?;
        if !pte.is_valid() {
            return None;
        }
        let pa = pte.phys_addr();
        *pte = PageTableEntry::empty();
        Some(pa)
    }

    /// Look up the PTE for `va`.
    pub fn lookup(&self, va: usize) -> Option<PageTableEntry> {
        let pte = self.walk(va)?;
        if pte.is_valid() {
            Some(*pte)
        } else {
            None
        }
    }

    /// Mutable access to the PTE for `va`, for flag updates (COW, mprotect).
    pub fn lookup_mut(&mut self, va: usize) -> Option<&'static mut PageTableEntry> {
        let pte = self.walk(va)?;
        if pte.is_valid() {
            Some(pte)
        } else {
            None
        }
    }

    /// Translate a virtual address to physical, honoring the page offset.
    pub fn translate(&self, va: usize) -> Option<usize> {
        let pte = self.lookup(page_down(va))?;
        Some(pte.phys_addr() + (va & PAGE_MASK))
    }

    /// Map a 1 GiB huge page (level-0 leaf). Used for the kernel identity map.
    pub fn map_gigapage(&mut self, va: usize, pa: usize, flags: PTEFlags) {
        debug_assert_eq!(va & ((1 << 30) - 1), 0);
        debug_assert_eq!(pa & ((1 << 30) - 1), 0);
        let idx = vpn_indices(va);
        Self::table_at(self.root)[idx[0]] = PageTableEntry::new(pa, flags | PTEFlags::V);
    }

    /// Copy the top-level entries that make up the kernel identity mapping from
    /// another table, so every address space shares the kernel half.
    pub fn clone_kernel_mappings(&mut self, from: &PageTable) {
        let src = Self::table_at(from.root);
        let dst = Self::table_at(self.root);
        // The kernel identity region occupies the first `USER_BASE >> 30`
        // gigapage slots of the root table.
        let kernel_slots = USER_BASE >> 30;
        for i in 0..kernel_slots {
            dst[i] = src[i];
        }
    }

    /// Iterate every valid leaf mapping in user space.
    pub fn for_each_user_leaf(&self, mut f: impl FnMut(usize, &'static mut PageTableEntry)) {
        let root = Self::table_at(self.root);
        let kernel_slots = USER_BASE >> 30;
        for i0 in kernel_slots..512 {
            if !root[i0].is_valid() || root[i0].is_leaf() {
                continue;
            }
            let l1 = Self::table_at(root[i0].phys_addr());
            for i1 in 0..512 {
                if !l1[i1].is_valid() || l1[i1].is_leaf() {
                    continue;
                }
                let l2 = Self::table_at(l1[i1].phys_addr());
                for i2 in 0..512 {
                    if l2[i2].is_valid() {
                        let va = (i0 << 30) | (i1 << 21) | (i2 << 12);
                        f(va, &mut l2[i2]);
                    }
                }
            }
        }
    }
}

impl Drop for PageTable {
    fn drop(&mut self) {
        // Leaf frames are released by AddrSpace before this runs; here we only
        // reclaim the table pages themselves.
        for &pa in &self.intermediate {
            frame::decref(pa);
        }
        frame::decref(self.root);
    }
}

/// Flush the entire TLB.
#[inline]
pub fn flush_tlb_all() {
    unsafe { core::arch::asm!("sfence.vma", options(nostack)) };
}

/// Flush a single virtual address from the TLB.
#[inline]
pub fn flush_tlb_page(va: usize) {
    unsafe { core::arch::asm!("sfence.vma {}", in(reg) va, options(nostack)) };
}

/// Install a page table into `satp` and flush the TLB.
#[inline]
pub fn activate(satp: usize) {
    unsafe {
        core::arch::asm!(
            "csrw satp, {}",
            "sfence.vma",
            in(reg) satp,
            options(nostack),
        );
    }
}
