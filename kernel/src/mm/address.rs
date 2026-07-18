//! Physical/virtual address and page-number newtypes for SV39.

use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};

const PA_WIDTH_SV39: usize = 56;
const VA_WIDTH_SV39: usize = 39;
const PPN_WIDTH_SV39: usize = PA_WIDTH_SV39 - PAGE_SIZE_BITS;
const VPN_WIDTH_SV39: usize = VA_WIDTH_SV39 - PAGE_SIZE_BITS;

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct PhysAddr(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct VirtAddr(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug, Hash)]
#[repr(transparent)]
pub struct PhysPageNum(pub usize);

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Debug, Hash)]
#[repr(transparent)]
pub struct VirtPageNum(pub usize);

impl From<usize> for PhysAddr {
    fn from(v: usize) -> Self {
        Self(v & ((1 << PA_WIDTH_SV39) - 1))
    }
}
impl From<usize> for VirtAddr {
    fn from(v: usize) -> Self {
        Self(v & ((1 << VA_WIDTH_SV39) - 1))
    }
}
impl From<usize> for PhysPageNum {
    fn from(v: usize) -> Self {
        Self(v & ((1 << PPN_WIDTH_SV39) - 1))
    }
}
impl From<usize> for VirtPageNum {
    fn from(v: usize) -> Self {
        Self(v & ((1 << VPN_WIDTH_SV39) - 1))
    }
}
impl From<PhysAddr> for usize {
    fn from(v: PhysAddr) -> Self {
        v.0
    }
}
impl From<VirtAddr> for usize {
    fn from(v: VirtAddr) -> Self {
        v.0
    }
}
impl From<PhysPageNum> for usize {
    fn from(v: PhysPageNum) -> Self {
        v.0
    }
}
impl From<VirtPageNum> for usize {
    fn from(v: VirtPageNum) -> Self {
        v.0
    }
}

impl PhysAddr {
    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }
    pub fn floor(&self) -> PhysPageNum {
        PhysPageNum(self.0 / PAGE_SIZE)
    }
    pub fn ceil(&self) -> PhysPageNum {
        PhysPageNum((self.0 + PAGE_SIZE - 1) / PAGE_SIZE)
    }
    pub fn is_aligned(&self) -> bool {
        self.page_offset() == 0
    }
    /// Kernel identity-maps all physical memory, so a physical address can be
    /// dereferenced directly as a pointer.
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.0 as *mut u8
    }
}

impl VirtAddr {
    pub fn page_offset(&self) -> usize {
        self.0 & (PAGE_SIZE - 1)
    }
    pub fn floor(&self) -> VirtPageNum {
        VirtPageNum(self.0 / PAGE_SIZE)
    }
    pub fn ceil(&self) -> VirtPageNum {
        VirtPageNum((self.0 + PAGE_SIZE - 1) / PAGE_SIZE)
    }
    pub fn is_aligned(&self) -> bool {
        self.page_offset() == 0
    }
    /// Like `page_offset`, but an address exactly on a page boundary maps
    /// to `PAGE_SIZE` rather than 0 -- useful as the exclusive end index
    /// into a page-sized byte slice.
    pub fn page_offset_or_full(&self) -> usize {
        match self.page_offset() {
            0 => PAGE_SIZE,
            off => off,
        }
    }
}

impl From<PhysAddr> for PhysPageNum {
    fn from(v: PhysAddr) -> Self {
        assert!(v.is_aligned());
        v.floor()
    }
}
impl From<PhysPageNum> for PhysAddr {
    fn from(v: PhysPageNum) -> Self {
        Self(v.0 * PAGE_SIZE)
    }
}
impl From<VirtAddr> for VirtPageNum {
    fn from(v: VirtAddr) -> Self {
        assert!(v.is_aligned());
        v.floor()
    }
}
impl From<VirtPageNum> for VirtAddr {
    fn from(v: VirtPageNum) -> Self {
        Self(v.0 * PAGE_SIZE)
    }
}

impl VirtPageNum {
    /// The three 9-bit indices used to walk a 3-level SV39 page table.
    pub fn indexes(&self) -> [usize; 3] {
        let mut vpn = self.0;
        let mut idx = [0usize; 3];
        for i in (0..3).rev() {
            idx[i] = vpn & 511;
            vpn >>= 9;
        }
        idx
    }
}

impl PhysPageNum {
    /// Kernel identity-maps all of physical memory, so we can get a direct
    /// slice/array view of a physical page's contents.
    pub fn as_bytes(&self) -> &'static mut [u8; PAGE_SIZE] {
        let pa: PhysAddr = (*self).into();
        unsafe { &mut *(pa.0 as *mut [u8; PAGE_SIZE]) }
    }
    pub fn as_pte_array(&self) -> &'static mut [super::page_table::PageTableEntry; 512] {
        let pa: PhysAddr = (*self).into();
        unsafe { &mut *(pa.0 as *mut [super::page_table::PageTableEntry; 512]) }
    }
    pub fn as_mut_ptr(&self) -> *mut u8 {
        let pa: PhysAddr = (*self).into();
        pa.as_mut_ptr()
    }
}
