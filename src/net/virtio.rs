//! Legacy (version 1) virtio-mmio network device driver, polled.
use crate::mm::frame;
use core::sync::atomic::{fence, Ordering};

const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_SLOTS: usize = 8;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;

const MAGIC: u32 = 0x7472_6976;
const DEV_NET: u32 = 1;

// registers
const REG_MAGIC: usize = 0x00;
const REG_VERSION: usize = 0x04;
const REG_DEVICE_ID: usize = 0x08;
const REG_HOST_FEATURES: usize = 0x10;
const REG_GUEST_FEATURES: usize = 0x20;
const REG_GUEST_PAGE_SIZE: usize = 0x28;
const REG_QUEUE_SEL: usize = 0x30;
const REG_QUEUE_NUM_MAX: usize = 0x34;
const REG_QUEUE_NUM: usize = 0x38;
const REG_QUEUE_ALIGN: usize = 0x3c;
const REG_QUEUE_PFN: usize = 0x40;
const REG_QUEUE_NOTIFY: usize = 0x50;
const REG_INT_STATUS: usize = 0x60;
const REG_INT_ACK: usize = 0x64;
const REG_STATUS: usize = 0x70;
const REG_CONFIG: usize = 0x100;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const F_MAC: u64 = 1 << 5;

const QSIZE: usize = 64;
const RX_BUF_LEN: usize = 2048;
const NET_HDR_LEN: usize = 10; // legacy, no MRG_RXBUF

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}
const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

struct Queue {
    base: usize,   // page-aligned region: desc | avail | pad | used
    buffers: [usize; QSIZE], // one frame per descriptor
    last_used: u16,
    avail_idx: u16,
    qsize: usize,
}

impl Queue {
    fn desc(&self, i: usize) -> *mut Desc {
        (self.base + i * 16) as *mut Desc
    }
    fn avail_flags(&self) -> *mut u16 {
        (self.base + 16 * self.qsize) as *mut u16
    }
    fn avail_idx_ptr(&self) -> *mut u16 {
        (self.base + 16 * self.qsize + 2) as *mut u16
    }
    fn avail_ring(&self, i: usize) -> *mut u16 {
        (self.base + 16 * self.qsize + 4 + i * 2) as *mut u16
    }
    fn used_base(&self) -> usize {
        // used ring starts at next page boundary after desc+avail
        let a = self.base + 16 * self.qsize + 4 + self.qsize * 2 + 2;
        (a + 4095) & !4095
    }
    fn used_idx(&self) -> u16 {
        unsafe { ((self.used_base() + 2) as *const u16).read_volatile() }
    }
    fn used_elem(&self, i: usize) -> (u32, u32) {
        unsafe {
            let p = (self.used_base() + 4 + i * 8) as *const u32;
            (p.read_volatile(), p.add(1).read_volatile())
        }
    }
}

pub struct VirtioNet {
    base: usize,
    rx: Queue,
    tx: Queue,
    pub mac: [u8; 6],
    tx_free: [bool; QSIZE],
}

fn mmio_r(base: usize, off: usize) -> u32 {
    unsafe { ((base + off) as *const u32).read_volatile() }
}
fn mmio_w(base: usize, off: usize, v: u32) {
    unsafe { ((base + off) as *mut u32).write_volatile(v) }
}

fn alloc_queue(base: usize, qidx: u32) -> Queue {
    mmio_w(base, REG_QUEUE_SEL, qidx);
    let max = mmio_r(base, REG_QUEUE_NUM_MAX) as usize;
    assert!(max >= QSIZE, "queue too small: {}", max);
    mmio_w(base, REG_QUEUE_NUM, QSIZE as u32);
    mmio_w(base, REG_QUEUE_ALIGN, 4096);

    // memory: desc(16*64=1024) + avail(4+128+2=134) → fits in 1 page; used on 2nd page
    let mem = frame::alloc();
    let mem2 = frame::alloc();
    assert_eq!(mem2, mem + 4096, "need contiguous frames for virtqueue");
    mmio_w(base, REG_QUEUE_PFN, (mem >> 12) as u32);

    let mut buffers = [0usize; QSIZE];
    for b in buffers.iter_mut() {
        *b = frame::alloc();
    }
    Queue {
        base: mem,
        buffers,
        last_used: 0,
        avail_idx: 0,
        qsize: QSIZE,
    }
}

pub fn probe() -> Option<VirtioNet> {
    for slot in 0..VIRTIO_MMIO_SLOTS {
        let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
        if mmio_r(base, REG_MAGIC) != MAGIC {
            continue;
        }
        if mmio_r(base, REG_DEVICE_ID) != DEV_NET {
            continue;
        }
        let version = mmio_r(base, REG_VERSION);
        println!("[net] virtio-net at {:#x}, version {}", base, version);
        assert_eq!(version, 1, "only legacy virtio-mmio supported");

        mmio_w(base, REG_STATUS, 0);
        mmio_w(base, REG_STATUS, STATUS_ACK);
        mmio_w(base, REG_STATUS, STATUS_ACK | STATUS_DRIVER);
        let feats = mmio_r(base, REG_HOST_FEATURES) as u64;
        let want = feats & F_MAC;
        mmio_w(base, REG_GUEST_FEATURES, want as u32);
        mmio_w(base, REG_GUEST_PAGE_SIZE, 4096);

        let rx = alloc_queue(base, 0);
        let tx = alloc_queue(base, 1);

        mmio_w(
            base,
            REG_STATUS,
            STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );

        let mut mac = [0u8; 6];
        for (i, m) in mac.iter_mut().enumerate() {
            *m = unsafe { ((base + REG_CONFIG + i) as *const u8).read_volatile() };
        }
        println!(
            "[net] mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );

        let mut dev = VirtioNet {
            base,
            rx,
            tx,
            mac,
            tx_free: [true; QSIZE],
        };
        dev.fill_rx();
        return Some(dev);
    }
    None
}

impl VirtioNet {
    fn fill_rx(&mut self) {
        for i in 0..QSIZE {
            unsafe {
                *self.rx.desc(i) = Desc {
                    addr: self.rx.buffers[i] as u64,
                    len: RX_BUF_LEN as u32,
                    flags: DESC_WRITE,
                    next: 0,
                };
                *self.rx.avail_ring(self.rx.avail_idx as usize % QSIZE) = i as u16;
            }
            self.rx.avail_idx = self.rx.avail_idx.wrapping_add(1);
        }
        unsafe {
            self.rx.avail_flags().write_volatile(0);
            fence(Ordering::SeqCst);
            self.rx.avail_idx_ptr().write_volatile(self.rx.avail_idx);
        }
        fence(Ordering::SeqCst);
        mmio_w(self.base, REG_QUEUE_NOTIFY, 0);
    }

    /// Receive one frame if available; returns (desc_id, frame bytes).
    pub fn recv(&mut self) -> Option<(usize, &[u8])> {
        fence(Ordering::SeqCst);
        if self.rx.used_idx() == self.rx.last_used {
            // ack any pending interrupt state (we poll)
            let int = mmio_r(self.base, REG_INT_STATUS);
            if int != 0 {
                mmio_w(self.base, REG_INT_ACK, int);
            }
            return None;
        }
        let (id, len) = self.rx.used_elem(self.rx.last_used as usize % QSIZE);
        self.rx.last_used = self.rx.last_used.wrapping_add(1);
        let buf = self.rx.buffers[id as usize];
        let frame = unsafe {
            core::slice::from_raw_parts(
                (buf + NET_HDR_LEN) as *const u8,
                len as usize - NET_HDR_LEN,
            )
        };
        Some((id as usize, frame))
    }

    /// Return an rx descriptor to the avail ring.
    pub fn recycle_rx(&mut self, id: usize) {
        unsafe {
            *self.rx.avail_ring(self.rx.avail_idx as usize % QSIZE) = id as u16;
            fence(Ordering::SeqCst);
            self.rx.avail_idx = self.rx.avail_idx.wrapping_add(1);
            self.rx.avail_idx_ptr().write_volatile(self.rx.avail_idx);
        }
        fence(Ordering::SeqCst);
        mmio_w(self.base, REG_QUEUE_NOTIFY, 0);
    }

    fn reclaim_tx(&mut self) {
        fence(Ordering::SeqCst);
        while self.tx.used_idx() != self.tx.last_used {
            let (id, _) = self.tx.used_elem(self.tx.last_used as usize % QSIZE);
            self.tx_free[id as usize] = true;
            self.tx.last_used = self.tx.last_used.wrapping_add(1);
        }
    }

    pub fn send(&mut self, frame: &[u8]) -> bool {
        self.reclaim_tx();
        let Some(id) = self.tx_free.iter().position(|&f| f) else {
            return false;
        };
        if NET_HDR_LEN + frame.len() > RX_BUF_LEN {
            return false;
        }
        self.tx_free[id] = false;
        let buf = self.tx.buffers[id];
        unsafe {
            core::ptr::write_bytes(buf as *mut u8, 0, NET_HDR_LEN);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), (buf + NET_HDR_LEN) as *mut u8, frame.len());
            *self.tx.desc(id) = Desc {
                addr: buf as u64,
                len: (NET_HDR_LEN + frame.len()) as u32,
                flags: 0,
                next: 0,
            };
            *self.tx.avail_ring(self.tx.avail_idx as usize % QSIZE) = id as u16;
            fence(Ordering::SeqCst);
            self.tx.avail_idx = self.tx.avail_idx.wrapping_add(1);
            self.tx.avail_idx_ptr().write_volatile(self.tx.avail_idx);
        }
        fence(Ordering::SeqCst);
        mmio_w(self.base, REG_QUEUE_NOTIFY, 1);
        true
    }
}
