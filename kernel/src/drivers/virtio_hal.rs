//! `virtio_drivers::Hal` implementation for our identity-mapped kernel:
//! any physical address is directly usable as a pointer, so most methods
//! are trivial.

use crate::mm::frame_allocator::{frame_alloc_contig, FrameTracker};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ptr::NonNull;
use spin::Mutex;
use virtio_drivers::{BufferDirection, Hal, PhysAddr as VirtioPhysAddr};

static DMA_REGISTRY: Mutex<BTreeMap<usize, Vec<FrameTracker>>> = Mutex::new(BTreeMap::new());

pub struct VirtioHalImpl;

unsafe impl Hal for VirtioHalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (VirtioPhysAddr, NonNull<u8>) {
        let frames = frame_alloc_contig(pages).expect("out of memory for virtio DMA");
        let paddr = frames[0].ppn.0 * crate::config::PAGE_SIZE;
        DMA_REGISTRY.lock().insert(paddr, frames);
        let vaddr = NonNull::new(paddr as *mut u8).unwrap();
        (paddr, vaddr)
    }

    unsafe fn dma_dealloc(paddr: VirtioPhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        DMA_REGISTRY.lock().remove(&paddr);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: VirtioPhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> VirtioPhysAddr {
        buffer.as_ptr() as *mut u8 as usize
    }

    unsafe fn unshare(_paddr: VirtioPhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}
