//! Virtio-net (MMIO, modern interface) driver with split virtqueues.

use crate::memory::frame::{self, PAGE_SIZE};
use alloc::vec::Vec;

pub const VIRTIO_NET_MMIO: usize = 0x1000_1000;

// MMIO register offsets
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;
const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG: usize = 0x100;

const DESC_F_WRITE: u16 = 2;

#[inline]
pub fn mmio_read(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((VIRTIO_NET_MMIO + off) as *const u32) }
}

#[inline]
pub fn mmio_write(off: usize, val: u32) {
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
pub const QUEUE_SIZE: usize = 64;

struct VirtQueue {
    base: usize,
    num: usize,
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
        let avail_size = 4 + self.num * 2 + 2;
        let off = self.num * DESC_SIZE + (avail_size + 1) & !1;
        (self.base + off) as *mut u16
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
        assert_eq!(mmio_read(MAGIC), 0x7472_6976, "virtio-net magic mismatch");
        assert_eq!(mmio_read(DEVICE_ID), 1, "not virtio-net");

        mmio_write(STATUS, 0);
        mmio_write(STATUS, 1);
        mmio_write(STATUS, 3);

        // Negotiate features: MAC (bit 5) + STATUS (bit 16).
        mmio_write(DRIVER_FEATURES_SEL, 0);
        mmio_write(DRIVER_FEATURES, (1 << 5) | (1 << 16));
        mmio_write(DRIVER_FEATURES_SEL, 1);
        mmio_write(DRIVER_FEATURES, 0);
        mmio_write(STATUS, 3 | 8);

        // Read MAC.
        let mut mac = [0u8; 6];
        loop {
            let gen = mmio_read(CONFIG_GENERATION);
            for i in 0..6 {
                mac[i] = mmio_read(CONFIG + i) as u8;
            }
            if gen == mmio_read(CONFIG_GENERATION) {
                break;
            }
        }

        let rx = Self::setup_queue(0, QUEUE_SIZE);
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
            }
        }

        mmio_write(QUEUE_SEL, 0);
        mmio_write(QUEUE_READY, 1);
        mmio_write(QUEUE_SEL, 1);
        mmio_write(QUEUE_READY, 1);
        mmio_write(STATUS, 3 | 8 | 4);

        let mut this = VirtioNet {
            rx,
            tx,
            rx_buffers,
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
        let num = size.min(max.max(1));
        mmio_write(QUEUE_NUM, num as u32);
        let base = frame::alloc().expect("out of frames for virtqueue");
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
        let buf_phys = self.rx_buffers[desc_idx as usize];
        let buf = unsafe { core::slice::from_raw_parts_mut(buf_phys as *mut u8, len as usize) };
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
