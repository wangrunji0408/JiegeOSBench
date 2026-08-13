//! Virtio-net (MMIO, modern interface) driver with split virtqueues.

use crate::memory::frame::{self, PAGE_SIZE};
use alloc::vec::Vec;

pub const VIRTIO_NET_MMIO: usize = 0x1000_1000;

// MMIO register offsets
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const VENDOR_ID: usize = 0x00c;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;
const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG: usize = 0x100;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[inline]
fn mmio_read(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((VIRTIO_NET_MMIO + off) as *const u32) }
}

#[inline]
fn mmio_write(off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((VIRTIO_NET_MMIO + off) as *mut u32, val) }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const DESC_SIZE: usize = 16;
const QUEUE_SIZE: usize = 64;

/// A split virtqueue with its three ring areas packed into one physical frame.
struct VirtQueue {
    /// Physical address of the ring frame.
    base: usize,
    /// Number of descriptors used (desc table occupies the rest of the frame).
    num: usize,
    /// Next available-ring index to write.
    avail_idx: u16,
    /// Last used-ring index we've processed.
    used_idx: u16,
}

impl VirtQueue {
    fn desc_table(&self) -> *mut Desc {
        self.base as *mut Desc
    }
    /// Available ring starts after the descriptor table.
    fn avail(&self) -> *mut u16 {
        (self.base + self.num * DESC_SIZE) as *mut u16
    }
    /// Used ring follows the available ring (aligned to 2 bytes).
    fn used(&self) -> *mut u16 {
        let avail_size = 4 + self.num * 2 + 2; // flags(2)+idx(2)+ring+used_event(2)
        let off = self.num * DESC_SIZE + (avail_size + 1) & !1;
        (self.base + off) as *mut u16
    }

    unsafe fn desc(&self, i: usize) -> &mut Desc {
        &mut *self.desc_table().add(i)
    }

    /// Add a descriptor chain (single descriptor for now) to the available ring.
    unsafe fn add_avail(&mut self, desc_idx: u16) {
        let avail = self.avail();
        // avail[0] = flags, avail[1] = idx, avail[2..] = ring
        let ring = avail.add(2);
        let idx = self.avail_idx as usize % self.num;
        *ring.add(idx) = desc_idx;
        // memory barrier
        core::arch::asm!("fence w, w");
        let new_idx = self.avail_idx.wrapping_add(1);
        *avail.add(1) = new_idx;
        self.avail_idx = new_idx;
    }

    /// Check the used ring for a completed element; returns (desc_idx, len) or None.
    unsafe fn get_used(&mut self) -> Option<(u16, u32)> {
        let used = self.used();
        let used_idx = *used.add(1);
        if used_idx == self.used_idx {
            return None;
        }
        // used ring layout: flags(2), idx(2), then ring of {id:u32,len:u32}
        let ring = (used as usize + 4) as *mut u32;
        let elem_idx = self.used_idx as usize % self.num;
        let id = *ring.add(elem_idx * 2);
        let len = *ring.add(elem_idx * 2 + 1);
        self.used_idx = self.used_idx.wrapping_add(1);
        Some((id as u16, len))
    }
}

pub struct VirtioNet {
    rx: VirtQueue,
    tx: VirtQueue,
    /// Physical address of each RX buffer (2048 bytes each, one per descriptor).
    rx_buffers: Vec<usize>,
    rx_buffer_size: usize,
    mac: [u8; 6],
}

impl VirtioNet {
    pub fn init() -> Self {
        // Verify magic and device id.
        assert_eq!(mmio_read(MAGIC), 0x7472_6976, "virtio-net magic mismatch");
        assert_eq!(mmio_read(DEVICE_ID), 1, "device is not virtio-net");

        // Reset.
        mmio_write(STATUS, 0);
        // Acknowledge.
        mmio_write(STATUS, 1);
        // Driver.
        mmio_write(STATUS, 3);

        // Negotiate features: MAC + STATUS.
        mmio_write(DEVICE_FEATURES_SEL, 0);
        let _feat0 = mmio_read(DEVICE_FEATURES);
        let driver_features: u32 = (1 << 5) | (1 << 16); // VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS
        mmio_write(DRIVER_FEATURES_SEL, 0);
        mmio_write(DRIVER_FEATURES, driver_features);
        mmio_write(DRIVER_FEATURES_SEL, 1);
        mmio_write(DRIVER_FEATURES, 0);
        // Features OK.
        mmio_write(STATUS, 3 | 8);

        // Read MAC address from config.
        let mut mac = [0u8; 6];
        let mut generation;
        loop {
            generation = mmio_read(CONFIG_GENERATION);
            for i in 0..6 {
                mac[i] = mmio_read(CONFIG + i) as u8;
            }
            if generation == mmio_read(CONFIG_GENERATION) {
                break;
            }
        }

        // Set up RX queue (0).
        let rx = Self::setup_queue(0, QUEUE_SIZE);
        // Set up TX queue (1).
        let tx = Self::setup_queue(1, QUEUE_SIZE);

        // Allocate RX buffers and fill the RX available ring.
        let mut rx_buffers = Vec::with_capacity(QUEUE_SIZE);
        let rx_buffer_size = 2048;
        for i in 0..QUEUE_SIZE {
            let buf = frame::alloc().expect("out of frames for RX buffers");
            rx_buffers.push(buf.0);
            unsafe {
                let d = rx.desc(i);
                d.addr = buf.0 as u64;
                d.len = rx_buffer_size as u32;
                d.flags = DESC_F_WRITE;
                d.next = 0;
            }
        }
        // Mark RX queue ready.
        mmio_write(QUEUE_SEL, 0);
        mmio_write(QUEUE_READY, 1);
        // Mark TX queue ready.
        mmio_write(QUEUE_SEL, 1);
        mmio_write(QUEUE_READY, 1);

        // Driver OK.
        mmio_write(STATUS, 3 | 8 | 4);

        // Add all RX descriptors to the available ring.
        let mut this = VirtioNet {
            rx,
            tx,
            rx_buffers,
            rx_buffer_size,
            mac,
        };
        for i in 0..QUEUE_SIZE {
            unsafe { this.rx.add_avail(i as u16); }
        }
        this
    }

    fn setup_queue(queue_sel: u32, size: usize) -> VirtQueue {
        mmio_write(QUEUE_SEL, queue_sel);
        let max = mmio_read(QUEUE_NUM_MAX) as usize;
        let num = size.min(max);
        mmio_write(QUEUE_NUM, num as u32);

        // Allocate one frame for descriptor table + rings.
        let base = frame::alloc().expect("out of frames for virtqueue");
        // Clear the frame.
        unsafe { core::slice::from_raw_parts_mut(base.0 as *mut u8, PAGE_SIZE).fill(0); }

        mmio_write(QUEUE_DESC_LOW, base.0 as u32);
        mmio_write(QUEUE_DESC_HIGH, 0);
        mmio_write(QUEUE_DRIVER_LOW, (base.0 + num * DESC_SIZE) as u32);
        mmio_write(QUEUE_DRIVER_HIGH, 0);
        let avail_size = 4 + num * 2 + 2;
        let used_off = num * DESC_SIZE + (avail_size + 1) & !1;
        mmio_write(QUEUE_DEVICE_LOW, (base.0 + used_off) as u32);
        mmio_write(QUEUE_DEVICE_HIGH, 0);

        VirtQueue {
            base: base.0,
            num,
            avail_idx: 0,
            used_idx: 0,
        }
    }

    fn mac(&self) -> [u8; 6] {
        self.mac
    }
}

// ---- smoltcp Device implementation ----

use smoltcp::phy::{DeviceCapabilities, Medium};
use smoltcp::time::Instant;

pub struct RxToken<'a> {
    net: &'a mut VirtioNet,
    buf: &'a mut [u8],
    desc_idx: u16,
}

impl smoltcp::phy::RxToken for RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let res = f(self.buf);
        // Recycle the RX buffer: re-add to available ring.
        unsafe {
            self.net.rx.add_avail(self.desc_idx);
        }
        res
    }
}

pub struct TxToken<'a> {
    net: &'a mut VirtioNet,
    buf: &'a mut [u8],
    used: bool,
}

impl smoltcp::phy::TxToken for TxToken<'_> {
    fn consume<R, F>(mut self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let res = f(&mut self.buf[..len]);
        // Add the TX descriptor and notify.
        unsafe {
            let idx = 0usize; // reuse descriptor 0 for TX (single packet in flight)
            let d = self.net.tx.desc(idx);
            d.addr = self.buf.as_ptr() as u64;
            d.len = len as u32;
            d.flags = 0;
            d.next = 0;
            self.net.tx.add_avail(idx as u16);
            // notify TX queue
            crate::net::virtio::mmio_write(QUEUE_SEL, 1);
            crate::net::virtio::mmio_write(QUEUE_NOTIFY, 1);
        }
        self.used = true;
        res
    }
}

impl Drop for TxToken<'_> {
    fn drop(&mut self) {
        // If not consumed, do nothing (buffer is static, will be reused).
        let _ = self.used;
    }
}

// A static TX buffer (contiguous, used one packet at a time).
#[repr(align(16))]
struct TxBuf([u8; 2048]);
static mut TX_BUF: TxBuf = TxBuf([0; 2048]);

impl<'a> smoltcp::phy::Device for VirtioNet {
    type RxToken<'x> = RxToken<'x> where Self: 'x;
    type TxToken<'x> = TxToken<'x> where Self: 'x;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Check RX used ring.
        let (desc_idx, len) = unsafe { self.rx.get_used()? };
        let buf_phys = self.rx_buffers[desc_idx as usize];
        let buf = unsafe {
            core::slice::from_raw_parts_mut(buf_phys as *mut u8, len as usize)
        };
        let tx_buf = unsafe { &mut TX_BUF.0 };
        let tx = TxToken { net: self, buf: tx_buf, used: false };
        let rx = RxToken { net: self, buf, desc_idx };
        Some((rx, tx))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let buf = unsafe { &mut TX_BUF.0 };
        Some(TxToken { net: self, buf, used: false })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ethernet;
        caps
    }
}
