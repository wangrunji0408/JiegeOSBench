use crate::config::{PAGE_SIZE, PAGE_SIZE_BITS};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PhysAddr(pub usize);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct VirtAddr(pub usize);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct PhysPageNum(pub usize);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct VirtPageNum(pub usize);

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
    pub fn aligned(&self) -> bool {
        self.page_offset() == 0
    }
}

impl From<PhysAddr> for PhysPageNum {
    fn from(v: PhysAddr) -> Self {
        assert_eq!(v.page_offset(), 0);
        v.floor()
    }
}

impl From<PhysPageNum> for PhysAddr {
    fn from(v: PhysPageNum) -> Self {
        PhysAddr(v.0 << PAGE_SIZE_BITS)
    }
}

impl From<VirtAddr> for VirtPageNum {
    fn from(v: VirtAddr) -> Self {
        assert_eq!(v.page_offset(), 0);
        v.floor()
    }
}

impl From<VirtPageNum> for VirtAddr {
    fn from(v: VirtPageNum) -> Self {
        VirtAddr(v.0 << PAGE_SIZE_BITS)
    }
}

impl PhysPageNum {
    /// 内核恒等映射下，物理页号即物理地址，可直接当指针用
    pub fn as_ptr<T>(&self) -> *mut T {
        PhysAddr::from(*self).0 as *mut T
    }
    pub fn as_bytes(&self) -> &'static mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.as_ptr(), PAGE_SIZE) }
    }
    pub fn as_pte_array(&self) -> &'static mut [crate::mm::page_table::PageTableEntry] {
        unsafe { core::slice::from_raw_parts_mut(self.as_ptr(), 512) }
    }
}

impl VirtPageNum {
    /// Sv39: 三级索引，从高到低
    pub fn indexes(&self) -> [usize; 3] {
        let mut vpn = self.0;
        let mut idx = [0usize; 3];
        for i in (0..3).rev() {
            idx[i] = vpn & 0x1ff;
            vpn >>= 9;
        }
        idx
    }
}
