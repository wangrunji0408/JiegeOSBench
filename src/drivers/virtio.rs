//! virtio over MMIO: device discovery and virtqueues.
//!
//! Implements the modern (version 2) virtio-mmio interface that QEMU's `virt`
//! machine exposes at 0x1000_1000..0x1000_9000.

use crate::mm::{self, PAGE_SIZE};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};

/// The `virt` machine has 8 virtio-mmio slots, 0x1000 bytes apart, counting
/// down from the highest address.
const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;
const VIRTIO_MMIO_COUNT: usize = 8;
/// The `virt` machine's virtio IRQs start at 1 and run in slot order.
const VIRTIO_IRQ_BASE: u32 = 1;

const MAGIC: u32 = 0x7472_6976; // "virt"

/// MMIO register offsets.
mod reg {
    pub const MAGIC_VALUE: usize = 0x000;
    pub const VERSION: usize = 0x004;
    pub const DEVICE_ID: usize = 0x008;
    pub const VENDOR_ID: usize = 0x00c;
    pub const DEVICE_FEATURES: usize = 0x010;
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    pub const DRIVER_FEATURES: usize = 0x020;
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    pub const QUEUE_SEL: usize = 0x030;
    pub const QUEUE_NUM_MAX: usize = 0x034;
    pub const QUEUE_NUM: usize = 0x038;
    pub const QUEUE_READY: usize = 0x044;
    pub const QUEUE_NOTIFY: usize = 0x050;
    pub const INTERRUPT_STATUS: usize = 0x060;
    pub const INTERRUPT_ACK: usize = 0x064;
    pub const STATUS: usize = 0x070;
    pub const QUEUE_DESC_LOW: usize = 0x080;
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    pub const QUEUE_DRIVER_LOW: usize = 0x090;
    pub const QUEUE_DRIVER_HIGH: usize = 0x094;
    pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
    pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;
    pub const CONFIG: usize = 0x100;
}

/// Device status bits.
mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const DRIVER_OK: u32 = 4;
    pub const FEATURES_OK: u32 = 8;
}

/// Feature bits we care about.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
pub const VIRTIO_F_RING_EVENT_IDX: u64 = 1 << 29;
pub const VIRTIO_F_RING_INDIRECT_DESC: u64 = 1 << 28;

/// Device IDs.
pub const DEVICE_ID_NET: u32 = 1;
pub const DEVICE_ID_BLOCK: u32 = 2;

/// A virtio-mmio transport.
pub struct MmioTransport {
    base: usize,
    pub irq: u32,
}

impl MmioTransport {
    #[inline]
    pub fn read(&self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    #[inline]
    pub fn write(&self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, value) };
    }

    /// Read a byte from the device configuration space.
    pub fn read_config_u8(&self, offset: usize) -> u8 {
        unsafe { read_volatile((self.base + reg::CONFIG + offset) as *const u8) }
    }

    pub fn read_config_u16(&self, offset: usize) -> u16 {
        unsafe { read_volatile((self.base + reg::CONFIG + offset) as *const u16) }
    }

    pub fn device_id(&self) -> u32 {
        self.read(reg::DEVICE_ID)
    }

    /// Read the 64-bit device feature set.
    pub fn device_features(&self) -> u64 {
        self.write(reg::DEVICE_FEATURES_SEL, 0);
        let low = self.read(reg::DEVICE_FEATURES) as u64;
        self.write(reg::DEVICE_FEATURES_SEL, 1);
        let high = self.read(reg::DEVICE_FEATURES) as u64;
        low | (high << 32)
    }

    fn set_driver_features(&self, features: u64) {
        self.write(reg::DRIVER_FEATURES_SEL, 0);
        self.write(reg::DRIVER_FEATURES, features as u32);
        self.write(reg::DRIVER_FEATURES_SEL, 1);
        self.write(reg::DRIVER_FEATURES, (features >> 32) as u32);
    }

    /// Run the device initialization handshake, negotiating `wanted` features.
    /// Returns the accepted feature set.
    pub fn begin_init(&self, wanted: u64) -> Result<u64, &'static str> {
        // Reset.
        self.write(reg::STATUS, 0);
        self.write(reg::STATUS, status::ACKNOWLEDGE);
        self.write(reg::STATUS, status::ACKNOWLEDGE | status::DRIVER);

        let device_features = self.device_features();
        let accepted = device_features & wanted;
        if accepted & VIRTIO_F_VERSION_1 == 0 {
            return Err("device does not support virtio 1.0");
        }
        self.set_driver_features(accepted);
        self.write(
            reg::STATUS,
            status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK,
        );
        if self.read(reg::STATUS) & status::FEATURES_OK == 0 {
            return Err("device rejected our feature set");
        }
        Ok(accepted)
    }

    pub fn finish_init(&self) {
        self.write(
            reg::STATUS,
            status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
        );
    }

    /// Configure a virtqueue and tell the device where its rings live.
    pub fn setup_queue(&self, index: u32, queue: &VirtQueue) -> Result<(), &'static str> {
        self.write(reg::QUEUE_SEL, index);
        if self.read(reg::QUEUE_READY) != 0 {
            return Err("queue already in use");
        }
        let max = self.read(reg::QUEUE_NUM_MAX);
        if max == 0 {
            return Err("queue not available");
        }
        if queue.size as u32 > max {
            return Err("queue size exceeds device maximum");
        }
        self.write(reg::QUEUE_NUM, queue.size as u32);

        let desc = queue.desc_paddr();
        let avail = queue.avail_paddr();
        let used = queue.used_paddr();
        self.write(reg::QUEUE_DESC_LOW, desc as u32);
        self.write(reg::QUEUE_DESC_HIGH, (desc >> 32) as u32);
        self.write(reg::QUEUE_DRIVER_LOW, avail as u32);
        self.write(reg::QUEUE_DRIVER_HIGH, (avail >> 32) as u32);
        self.write(reg::QUEUE_DEVICE_LOW, used as u32);
        self.write(reg::QUEUE_DEVICE_HIGH, (used >> 32) as u32);
        self.write(reg::QUEUE_READY, 1);
        Ok(())
    }

    pub fn notify(&self, queue: u32) {
        self.write(reg::QUEUE_NOTIFY, queue);
    }

    /// Read and acknowledge the interrupt status. Returns the bits that were set.
    pub fn ack_interrupt(&self) -> u32 {
        let status = self.read(reg::INTERRUPT_STATUS);
        if status != 0 {
            self.write(reg::INTERRUPT_ACK, status);
        }
        status
    }
}

// ---------------------------------------------------------------------------
// Virtqueue
// ---------------------------------------------------------------------------

/// A descriptor in the descriptor table.
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

pub const VRING_DESC_F_NEXT: u16 = 1;
pub const VRING_DESC_F_WRITE: u16 = 2;

/// The available ring header.
#[repr(C)]
struct AvailRing {
    flags: u16,
    idx: u16,
    // ring: [u16; size]
    // used_event: u16
}

/// One entry in the used ring.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

/// The used ring header.
#[repr(C)]
struct UsedRing {
    flags: u16,
    idx: u16,
    // ring: [UsedElem; size]
    // avail_event: u16
}

pub const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;

/// A split virtqueue.
///
/// The three rings are laid out in one contiguous physically-allocated block, in
/// the order and with the alignment the spec requires.
pub struct VirtQueue {
    /// Physical (== virtual, identity mapped) base of the ring memory.
    base: usize,
    /// Total size of the allocation, in bytes.
    alloc_size: usize,
    pub size: u16,
    /// Offsets from `base`.
    avail_offset: usize,
    used_offset: usize,
    /// Head of the free descriptor list.
    free_head: u16,
    /// Number of free descriptors.
    num_free: u16,
    /// Our copy of the avail ring index.
    avail_idx: u16,
    /// The last used index we have processed.
    last_used_idx: u16,
}

impl VirtQueue {
    /// Allocate a queue with `size` descriptors. `size` must be a power of two.
    pub fn new(size: u16) -> Option<Self> {
        assert!(size.is_power_of_two(), "virtqueue size must be a power of 2");
        let n = size as usize;

        let desc_bytes = n * core::mem::size_of::<Descriptor>();
        // avail: flags + idx + ring[n] + used_event
        let avail_bytes = 2 + 2 + n * 2 + 2;
        // The used ring must be 4-byte aligned; align its start to 4.
        let avail_offset = desc_bytes;
        let used_offset = (avail_offset + avail_bytes + 3) & !3;
        // used: flags + idx + ring[n] + avail_event
        let used_bytes = 2 + 2 + n * core::mem::size_of::<UsedElem>() + 2;
        let total = used_offset + used_bytes;

        // Allocate whole pages so the memory is physically contiguous (our frame
        // allocator hands out sequential frames, but relying on that is fragile,
        // so allocate one block of pages at once from the frame allocator by
        // requesting contiguous frames).
        let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;
        let base = alloc_contiguous(pages)?;
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, pages * PAGE_SIZE);
        }

        let mut queue = Self {
            base,
            alloc_size: pages * PAGE_SIZE,
            size,
            avail_offset,
            used_offset,
            free_head: 0,
            num_free: size,
            avail_idx: 0,
            last_used_idx: 0,
        };
        // Thread the free list through the `next` fields.
        for i in 0..n {
            let desc = queue.desc_mut(i as u16);
            desc.next = if i + 1 < n { (i + 1) as u16 } else { 0 };
        }
        Some(queue)
    }

    pub fn desc_paddr(&self) -> u64 {
        mm::virt_to_phys(self.base) as u64
    }

    pub fn avail_paddr(&self) -> u64 {
        mm::virt_to_phys(self.base + self.avail_offset) as u64
    }

    pub fn used_paddr(&self) -> u64 {
        mm::virt_to_phys(self.base + self.used_offset) as u64
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn desc_mut(&self, index: u16) -> &mut Descriptor {
        unsafe { &mut *((self.base as *mut Descriptor).add(index as usize)) }
    }

    #[inline]
    fn avail(&self) -> *mut AvailRing {
        (self.base + self.avail_offset) as *mut AvailRing
    }

    #[inline]
    fn avail_ring(&self) -> *mut u16 {
        (self.base + self.avail_offset + 4) as *mut u16
    }

    #[inline]
    fn used(&self) -> *const UsedRing {
        (self.base + self.used_offset) as *const UsedRing
    }

    #[inline]
    fn used_ring(&self) -> *const UsedElem {
        (self.base + self.used_offset + 4) as *const UsedElem
    }

    /// Suppress used-ring interrupts (we poll instead in some paths).
    pub fn set_no_interrupt(&self, suppress: bool) {
        unsafe {
            let flags = if suppress { VRING_AVAIL_F_NO_INTERRUPT } else { 0 };
            write_volatile(core::ptr::addr_of_mut!((*self.avail()).flags), flags);
        }
    }

    /// Add a buffer chain to the queue. `readable` buffers are device-readable
    /// (driver output), `writable` are device-writable (driver input).
    ///
    /// Returns the head descriptor index, which identifies the request in the
    /// used ring.
    pub fn add(&mut self, readable: &[(usize, usize)], writable: &[(usize, usize)]) -> Option<u16> {
        let count = readable.len() + writable.len();
        if count == 0 || count > self.num_free as usize {
            return None;
        }

        // Unlink the descriptors we need from the free list *before* writing
        // anything, so overwriting a descriptor's `next` can't corrupt the list.
        let head = self.free_head;
        let mut chain = [0u16; 8];
        let chain = if count <= chain.len() {
            &mut chain[..count]
        } else {
            // Chains longer than 8 descriptors don't occur in this driver (the
            // longest is a header plus a frame), so refuse rather than allocate.
            return None;
        };
        let mut cursor = head;
        for slot in chain.iter_mut() {
            *slot = cursor;
            cursor = self.desc_mut(cursor).next;
        }
        self.free_head = cursor;
        self.num_free -= count as u16;

        // Now fill them in, linking each to the next member of the chain.
        for (i, &(addr, len)) in readable.iter().chain(writable.iter()).enumerate() {
            let is_write = i >= readable.len();
            let desc = self.desc_mut(chain[i]);
            desc.addr = mm::virt_to_phys(addr) as u64;
            desc.len = len as u32;
            desc.flags = if is_write { VRING_DESC_F_WRITE } else { 0 };
            if i + 1 < count {
                desc.flags |= VRING_DESC_F_NEXT;
                desc.next = chain[i + 1];
            } else {
                desc.next = 0;
            }
        }

        // Publish the head in the avail ring.
        let ring_index = self.avail_idx % self.size;
        unsafe {
            write_volatile(self.avail_ring().add(ring_index as usize), head);
        }
        self.avail_idx = self.avail_idx.wrapping_add(1);
        // The device must see the descriptors before the index update.
        fence(Ordering::SeqCst);
        unsafe {
            write_volatile(core::ptr::addr_of_mut!((*self.avail()).idx), self.avail_idx);
        }
        fence(Ordering::SeqCst);
        Some(head)
    }

    /// Is there a completed request waiting?
    pub fn can_pop(&self) -> bool {
        // The device writes `used.idx` last, after the ring entry it describes.
        // An acquire fence after the read pairs with that, so the entry we go on
        // to read is the one the index promises.
        let used_idx = unsafe { read_volatile(core::ptr::addr_of!((*self.used()).idx)) };
        fence(Ordering::Acquire);
        used_idx != self.last_used_idx
    }

    /// How many completions are pending.
    pub fn pending(&self) -> u16 {
        let used_idx = unsafe { read_volatile(core::ptr::addr_of!((*self.used()).idx)) };
        fence(Ordering::Acquire);
        used_idx.wrapping_sub(self.last_used_idx)
    }

    /// Take one completed request, returning (head descriptor, bytes written).
    pub fn pop(&mut self) -> Option<(u16, u32)> {
        if !self.can_pop() {
            return None;
        }
        let index = self.last_used_idx % self.size;
        let elem = unsafe { read_volatile(self.used_ring().add(index as usize)) };
        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        let head = elem.id as u16;
        self.free_chain(head);
        Some((head, elem.len))
    }

    /// Return a descriptor chain to the free list.
    fn free_chain(&mut self, head: u16) {
        let mut current = head;
        let mut count = 0u16;
        loop {
            let desc = self.desc_mut(current);
            let has_next = desc.flags & VRING_DESC_F_NEXT != 0;
            let next = desc.next;
            desc.addr = 0;
            desc.len = 0;
            desc.flags = 0;
            count += 1;
            if !has_next {
                // Link the tail back into the free list.
                desc.next = self.free_head;
                break;
            }
            current = next;
        }
        self.free_head = head;
        self.num_free += count;
    }

    pub fn free_count(&self) -> u16 {
        self.num_free
    }
}

impl Drop for VirtQueue {
    fn drop(&mut self) {
        let pages = self.alloc_size / PAGE_SIZE;
        for i in 0..pages {
            mm::frame::decref(self.base + i * PAGE_SIZE);
        }
    }
}

/// Allocate `pages` physically contiguous frames.
///
/// Our frame allocator hands out frames from a bump pointer at boot, so a run of
/// allocations is contiguous in practice; we verify it and retry rather than
/// assume.
fn alloc_contiguous(pages: usize) -> Option<usize> {
    if pages == 1 {
        return mm::frame::alloc_frame();
    }
    for _attempt in 0..16 {
        let mut frames = alloc::vec::Vec::with_capacity(pages);
        let mut contiguous = true;
        for i in 0..pages {
            let pa = mm::frame::alloc_frame()?;
            if i > 0 && pa != frames[i - 1] + PAGE_SIZE {
                contiguous = false;
            }
            frames.push(pa);
        }
        if contiguous {
            return Some(frames[0]);
        }
        // Release and try again; the freed frames go on the recycle list, so the
        // next attempt draws from the bump pointer instead.
        for pa in frames {
            mm::frame::decref(pa);
        }
    }
    None
}

/// Scan the MMIO slots and initialize the devices we know about.
pub fn probe() {
    for slot in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
        let transport = MmioTransport {
            base,
            irq: VIRTIO_IRQ_BASE + slot as u32,
        };
        if transport.read(reg::MAGIC_VALUE) != MAGIC {
            continue;
        }
        let version = transport.read(reg::VERSION);
        let device_id = transport.device_id();
        if device_id == 0 {
            continue; // empty slot
        }
        crate::info!(
            "virtio-mmio slot {} at {:#x}: device {} version {} vendor {:#x} irq {}",
            slot,
            base,
            device_id,
            version,
            transport.read(reg::VENDOR_ID),
            transport.irq,
        );
        if version != 2 {
            crate::warn!("virtio: legacy version {} unsupported, skipping", version);
            continue;
        }

        match device_id {
            DEVICE_ID_NET => match super::virtio_net::init(transport) {
                Ok(()) => crate::info!("virtio-net initialized"),
                Err(e) => crate::warn!("virtio-net init failed: {}", e),
            },
            _ => {}
        }
    }
}
