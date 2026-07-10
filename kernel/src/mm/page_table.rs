use alloc::vec::Vec;
use bitflags::bitflags;
use crate::config::*;
use super::frame::{alloc_frame, FrameTracker, PhysFrame};

bitflags! {
    pub struct PTEFlags: u8 {
        const V = 1 << 0; // Valid
        const R = 1 << 1; // Readable
        const W = 1 << 2; // Writable
        const X = 1 << 3; // Executable
        const U = 1 << 4; // User mode
        const G = 1 << 5; // Global
        const A = 1 << 6; // Accessed
        const D = 1 << 7; // Dirty
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub fn new(ppn: usize, flags: PTEFlags) -> Self {
        Self(((ppn as u64) << 10) | (flags.bits() as u64))
    }

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn ppn(&self) -> usize {
        ((self.0 >> 10) & 0xFFF_FFFF_FFFF) as usize
    }

    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0 as u8)
    }

    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }

    pub fn is_leaf(&self) -> bool {
        let f = self.flags();
        f.intersects(PTEFlags::R | PTEFlags::W | PTEFlags::X)
    }

    pub fn phys_addr(&self) -> usize {
        self.ppn() << PAGE_SIZE_BITS
    }
}

pub struct PageTable {
    root_ppn: PhysFrame,
    frames: Vec<FrameTracker>, // 持有所有页表页
}

impl PageTable {
    pub fn new() -> Self {
        let root = alloc_frame().expect("no memory for page table");
        // Zero the root page table frame so all entries start as invalid
        // This is safe because new() is called with kernel page table active
        // (or in Bare mode before page tables are set up)
        root.0.zero();
        let root_ppn = PhysFrame::from_ppn(root.ppn());
        let mut frames = Vec::new();
        frames.push(root);
        Self { root_ppn, frames }
    }

    /// 从SATP寄存器创建（不拥有frames）
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysFrame::from_ppn(satp & ((1 << 44) - 1)),
            frames: Vec::new(),
        }
    }

    pub fn token(&self) -> usize {
        // Sv39: mode=8
        (8usize << 60) | self.root_ppn.ppn()
    }

    pub fn root_ppn(&self) -> PhysFrame {
        self.root_ppn
    }

    fn pte_array(ppn: PhysFrame) -> &'static mut [PageTableEntry] {
        let pa = crate::utils::phys_to_virt(ppn.addr());
        unsafe { core::slice::from_raw_parts_mut(pa as *mut PageTableEntry, 512) }
    }

    fn find_or_create_pte(&mut self, vpn: usize) -> Option<&mut PageTableEntry> {
        let indices = [
            (vpn >> 18) & 0x1FF,
            (vpn >> 9) & 0x1FF,
            vpn & 0x1FF,
        ];
        let mut ppn = self.root_ppn;
        let mut created_new = false;
        for i in 0..2 {
            let pte = &mut Self::pte_array(ppn)[indices[i]];
            if !pte.is_valid() {
                let frame = alloc_frame().expect("no memory for page table");
                // Frame is zeroed by FrameTracker::new (using kernel_satp if needed)
                *pte = PageTableEntry::new(frame.ppn(), PTEFlags::V);
                ppn = PhysFrame::from_ppn(frame.ppn());
                self.frames.push(frame);
                created_new = true;
            } else {
                ppn = PhysFrame::from_ppn(pte.ppn());
            }
        }
        // Flush TLB if we created new intermediate page table entries
        if created_new {
            unsafe { core::arch::asm!("sfence.vma"); }
        }
        Some(&mut Self::pte_array(ppn)[indices[2]])
    }

    fn find_pte(&self, vpn: usize) -> Option<&PageTableEntry> {
        let indices = [
            (vpn >> 18) & 0x1FF,
            (vpn >> 9) & 0x1FF,
            vpn & 0x1FF,
        ];
        let mut ppn = self.root_ppn;
        for i in 0..2 {
            let pte = &Self::pte_array(ppn)[indices[i]];
            if !pte.is_valid() {
                return None;
            }
            ppn = PhysFrame::from_ppn(pte.ppn());
        }
        let pte = &Self::pte_array(ppn)[indices[2]];
        if pte.is_valid() { Some(pte) } else { None }
    }

    pub fn map(&mut self, vpn: usize, ppn: usize, flags: PTEFlags) {
        let pte = self.find_or_create_pte(vpn).unwrap();
        // 设置A和D位，避免硬件访问fault
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V | PTEFlags::A | PTEFlags::D);
        unsafe { core::arch::asm!("sfence.vma"); }
    }

    /// 重新映射（允许覆盖已有映射）
    pub fn remap(&mut self, vpn: usize, ppn: usize, flags: PTEFlags) {
        let pte = self.find_or_create_pte(vpn).unwrap();
        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V | PTEFlags::A | PTEFlags::D);
        unsafe { core::arch::asm!("sfence.vma"); }
    }

    pub fn unmap(&mut self, vpn: usize) {
        let pte = self.find_or_create_pte(vpn).unwrap();
        assert!(pte.is_valid(), "vpn {:x} not mapped", vpn);
        *pte = PageTableEntry::empty();
    }

    pub fn translate(&self, vpn: usize) -> Option<PageTableEntry> {
        self.find_pte(vpn).copied()
    }

    /// 将虚拟地址转换为物理地址
    pub fn translate_va(&self, va: usize) -> Option<usize> {
        let vpn = va >> PAGE_SIZE_BITS;
        let offset = va & (PAGE_SIZE - 1);
        self.translate(vpn).map(|pte| (pte.ppn() << PAGE_SIZE_BITS) | offset)
    }

    /// 从用户空间读取数据
    pub fn copy_from_user(&self, src_va: usize, dst: &mut [u8]) {
        let mut copied = 0;
        let mut va = src_va;
        while copied < dst.len() {
            let vpn = va >> PAGE_SIZE_BITS;
            let offset = va & (PAGE_SIZE - 1);
            let pa = self.translate(vpn).unwrap().ppn() << PAGE_SIZE_BITS;
            let src_pa = crate::utils::phys_to_virt(pa + offset);
            let remaining = dst.len() - copied;
            let chunk = (PAGE_SIZE - offset).min(remaining);
            dst[copied..copied + chunk].copy_from_slice(unsafe {
                core::slice::from_raw_parts(src_pa as *const u8, chunk)
            });
            copied += chunk;
            va += chunk;
        }
    }

    /// 向用户空间写入数据
    pub fn copy_to_user(&self, dst_va: usize, src: &[u8]) {
        let mut copied = 0;
        let mut va = dst_va;
        while copied < src.len() {
            let vpn = va >> PAGE_SIZE_BITS;
            let offset = va & (PAGE_SIZE - 1);
            let pa = self.translate(vpn).unwrap().ppn() << PAGE_SIZE_BITS;
            let dst_pa = crate::utils::phys_to_virt(pa + offset);
            let remaining = src.len() - copied;
            let chunk = (PAGE_SIZE - offset).min(remaining);
            unsafe {
                core::slice::from_raw_parts_mut(dst_pa as *mut u8, chunk)
                    .copy_from_slice(&src[copied..copied + chunk]);
            }
            copied += chunk;
            va += chunk;
        }
    }

    /// 读取用户空间字符串
    pub fn read_cstr(&self, va: usize) -> alloc::string::String {
        let mut s = alloc::vec::Vec::new();
        let mut addr = va;
        loop {
            let pa = self.translate_va(addr).unwrap();
            let byte = unsafe { *(crate::utils::phys_to_virt(pa) as *const u8) };
            if byte == 0 { break; }
            s.push(byte);
            addr += 1;
        }
        alloc::string::String::from_utf8(s).unwrap_or_default()
    }
}
