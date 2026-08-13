//! Virtio-net (MMIO, legacy 0.9.5 interface) driver with split virtqueues.

use crate::memory::frame::{self, PAGE_SIZE};
use alloc::vec::Vec;

pub const VIRTIO_NET_MMIO: usize = 0x1000_1000;

// Legacy MMIO register offsets.
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const HOST_FEATURES: usize = 0x010;
const HOST_FEATURES_SEL: usize = 0x014;
const GUEST_FEATURES: usize = 0x020;
const GUEST_FEATURES_SEL: usize = 0x024;
const GUEST_PAGE_SIZE: usize = 0x028;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_ALIGN: usize = 0x03c;
const QUEUE_PFN: usize = 0x040;
const QUEUE_NOTIFY: usize = 0x050;
const STATUS: usize = 0x070;
const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG: usize = 0x100;

const DESC_F_WRITE: u16 = 2;
/// Legacy virtio-net prepends a 10-byte header to each packet.
const NET_HDR_SIZE: usize = 10;

pub static RX_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static TX_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

static MMIO_BASE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(VIRTIO_NET_MMIO);

#[inline]
pub fn mmio_read(off: usize) -> u32 {
    let base = MMIO_BASE.load(core::sync::atomic::Ordering::Relaxed);
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

#[inline]
pub fn mmio_write(off: usize, val: u32) {
    let base = MMIO_BASE.load(core::sync::atomic::Ordering::Relaxed);
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
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
pub const QUEUE_SIZE: usize = 64;

struct VirtQueue {
    base: usize,
    num: usize,
    used_off: usize,
    avail_idx: u16,
    used_idx: u16,
}

impl VirtQueue {
    fn desc_table(&self) -> *mut Desc {
        self.base as *mut Desc
    }
    fn avail(&self) -> *mut u16 {
        (self.base + self.num * DESC_SIZE) as *mut u16
    }
    fn used(&self) -> *mut u16 {
        (self.base + self.used_off) as *mut u16
    }

    unsafe fn desc(&self, i: usize) -> &mut Desc {
        &mut *self.desc_table().add(i)
    }

    unsafe fn add_avail(&mut self, desc_idx: u16) {
        let avail = self.avail();
        let ring = avail.add(2);
        let idx = self.avail_idx as usize % self.num;
        *ring.add(idx) = desc_idx;
        core::arch::asm!("fence w, w");
        let new_idx = self.avail_idx.wrapping_add(1);
        *avail.add(1) = new_idx;
        self.avail_idx = new_idx;
    }

    unsafe fn get_used(&mut self) -> Option<(u16, u32)> {
        let used = self.used();
        let used_idx = *used.add(1);
        if used_idx == self.used_idx {
            return None;
        }
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
    rx_buffers: Vec<usize>,
    mac: [u8; 6],
}

impl VirtioNet {
    pub fn init() -> Self {
        let mut base = 0usize;
        for slot in 0..8usize {
            let b = VIRTIO_NET_MMIO + slot * 0x1000;
            let magic = unsafe { core::ptr::read_volatile((b + MAGIC) as *const u32) };
            let devid = unsafe { core::ptr::read_volatile((b + DEVICE_ID) as *const u32) };
            crate::println!("[virtio] slot {}: magic={:#x} devid={}", slot, magic, devid);
            if magic == 0x7472_6976 && devid == 1 {
                base = b;
                break;
            }
        }
        assert_ne!(base, 0, "virtio-net device not found");
        MMIO_BASE.store(base, core::sync::atomic::Ordering::Relaxed);
        crate::println!("[virtio] virtio-net at {:#x} (legacy)", base);

        // Legacy init: reset, acknowledge, driver.
        mmio_write(STATUS, 0);
        mmio_write(STATUS, 1);
        mmio_write(STATUS, 3);

        // Negotiate features: MAC (bit 5).
        mmio_write(GUEST_FEATURES_SEL, 0);
        mmio_write(GUEST_FEATURES, 1 << 5);
        mmio_write(GUEST_FEATURES_SEL, 1);
        mmio_write(GUEST_FEATURES, 0);

        // Read MAC.
        let mut mac = [0u8; 6];
        loop {
            let gen = mmio_read(CONFIG_GENERATION);
            for i in 0..6 {
                mac[i] = unsafe { core::ptr::read_volatile((base + CONFIG + i) as *const u8) };
            }
            if gen == mmio_read(CONFIG_GENERATION) {
                break;
            }
        }

        // Set guest page size.
        mmio_write(GUEST_PAGE_SIZE, PAGE_SIZE as u32);

        let mut rx = Self::setup_queue(0, QUEUE_SIZE);
        let tx = Self::setup_queue(1, QUEUE_SIZE);

        let mut rx_buffers = Vec::with_capacity(QUEUE_SIZE);
        for i in 0..QUEUE_SIZE {
            let buf = frame::alloc().expect("out of frames for RX");
            rx_buffers.push(buf.0);
            unsafe {
                let d = rx.desc(i);
                d.addr = buf.0 as u64;
                d.len = 2048;
                d.flags = DESC_F_WRITE;
                d.next = 0;
                rx.add_avail(i as u16);
            }
        }

        // Driver OK.
        mmio_write(STATUS, 3 | 4);

        // Notify RX queue.
        mmio_write(QUEUE_SEL, 0);
        mmio_write(QUEUE_NOTIFY, 0);

        VirtioNet {
            rx,
            tx,
            rx_buffers,
            mac,
        }
    }

    fn setup_queue(queue_sel: u32, size: usize) -> VirtQueue {
        mmio_write(QUEUE_SEL, queue_sel);
        let max = mmio_read(QUEUE_NUM_MAX) as usize;
        let num = size.min(max.max(1));
        mmio_write(QUEUE_NUM, num as u32);
        mmio_write(QUEUE_ALIGN, PAGE_SIZE as u32);

        // The legacy used ring must be page-aligned, so the virtqueue spans
        // two pages for a 64-entry queue.
        let avail_size = 2 + 2 + 2 * num;
        let used_off = (num * DESC_SIZE + avail_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let used_size = 2 + 2 + 8 * num;
        let total = used_off + used_size;
        let pages = (total + PAGE_SIZE - 1) / PAGE_SIZE;

        let base = frame::alloc_contiguous(pages).expect("no contiguous frames for virtqueue");
        unsafe { core::slice::from_raw_parts_mut(base.0 as *mut u8, pages * PAGE_SIZE).fill(0); }

        // Legacy: the device is given the physical page number of the queue.
        mmio_write(QUEUE_PFN, (base.0 / PAGE_SIZE) as u32);

        VirtQueue {
            base: base.0,
            num,
            used_off,
            avail_idx: 0,
            used_idx: 0,
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }
}

use smoltcp::phy::{DeviceCapabilities, Medium};
use smoltcp::time::Instant;

pub struct RxToken<'a> {
    net: *mut VirtioNet,
    buf: &'a mut [u8],
    desc_idx: u16,
}

impl smoltcp::phy::RxToken for RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let res = f(self.buf);
        unsafe {
            (&mut *self.net).rx.add_avail(self.desc_idx);
            mmio_write(QUEUE_SEL, 0);
            mmio_write(QUEUE_NOTIFY, 0);
        }
        res
    }
}

pub struct TxToken<'a> {
    net: *mut VirtioNet,
    buf: &'a mut [u8],
}

impl smoltcp::phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let res = f(&mut self.buf[..len]);
        unsafe {
            let net = &mut *self.net;
            let idx = 0usize;
            let d = net.tx.desc(idx);
            d.addr = self.buf.as_ptr() as u64;
            d.len = len as u32;
            d.flags = 0;
            d.next = 0;
            net.tx.add_avail(idx as u16);
            TX_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            mmio_write(QUEUE_SEL, 1);
            mmio_write(QUEUE_NOTIFY, 1);
        }
        res
    }
}

#[repr(align(16))]
struct TxBuf([u8; 2048]);
static mut TX_BUF: TxBuf = TxBuf([0; 2048]);

impl smoltcp::phy::Device for VirtioNet {
    type RxToken<'x> = RxToken<'x> where Self: 'x;
    type TxToken<'x> = TxToken<'x> where Self: 'x;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let (desc_idx, len) = unsafe { self.rx.get_used()? };
        let n = RX_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n == 0 {
            let buf_phys = self.rx_buffers[desc_idx as usize];
            let head: [u8; 16] = unsafe { core::ptr::read((buf_phys as *const u8) as *const [u8; 16]) };
            crate::println!("[virtio] first RX len={} head={:02x?}", len, head);
        }
        let buf_phys = self.rx_buffers[desc_idx as usize];
        // Skip the 10-byte legacy header.
        let buf = unsafe {
            core::slice::from_raw_parts_mut((buf_phys + NET_HDR_SIZE) as *mut u8, (len as usize).saturating_sub(NET_HDR_SIZE))
        };
        let tx_buf = unsafe { &mut TX_BUF.0 };
        let net_ptr = self as *mut VirtioNet;
        Some((
            RxToken { net: net_ptr, buf, desc_idx },
            TxToken { net: net_ptr, buf: tx_buf },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let buf = unsafe { &mut TX_BUF.0 };
        Some(TxToken { net: self as *mut VirtioNet, buf })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ethernet;
        caps
    }
}
