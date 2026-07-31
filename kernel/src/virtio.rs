//! virtio 1.0 (MMIO) network driver for QEMU virt at 0x10001000.

use core::arch::asm;

use crate::mm::frame;
use crate::mm::paging::PAGE_SIZE;
use crate::net::OUR_MAC;

const VIRTIO_BASE: usize = 0x1000_1000;
const QUEUE_SIZE: usize = 64;

// registers
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x028;
const QUEUE_NUM_MAX: usize = 0x02c;
const QUEUE_NUM: usize = 0x030;
const QUEUE_READY: usize = 0x034;
const QUEUE_DESC_LOW: usize = 0x044;
const QUEUE_DESC_HIGH: usize = 0x048;
const QUEUE_DRIVER_LOW: usize = 0x050;
const QUEUE_DRIVER_HIGH: usize = 0x054;
const QUEUE_DEVICE_LOW: usize = 0x058;
const QUEUE_DEVICE_HIGH: usize = 0x05c;
const QUEUE_NOTIFY: usize = 0x060;
const INTERRUPT_STATUS: usize = 0x064;
const INTERRUPT_ACK: usize = 0x068;
const STATUS: usize = 0x070;
const CONFIG: usize = 0x100;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

fn reg32(off: usize) -> u32 {
    unsafe { ((VIRTIO_BASE + off) as *const u32).read_volatile() }
}

fn wreg32(off: usize, v: u32) {
    unsafe {
        ((VIRTIO_BASE + off) as *mut u32).write_volatile(v);
    }
}

fn reg64(off: usize) -> u64 {
    unsafe { ((VIRTIO_BASE + off) as *const u64).read_volatile() }
}

fn wreg64(off: usize, v: u64) {
    unsafe {
        ((VIRTIO_BASE + off) as *mut u64).write_volatile(v);
    }
}

// virtqueue
struct Vq {
    desc: usize,  // physical address of descriptor table
    avail: usize, // available ring
    used: usize,  // used ring
    free: Vec<usize>, // free descriptor indices (for TX)
    next_free: usize,
}

const VIRTIO_NET_HDR: usize = 10;

static mut RX_QUEUE: Option<Vq> = None;
static mut TX_QUEUE: Option<Vq> = None;
// RX: descriptor index i uses buffer rx_bufs[i]
static mut RX_BUFS: [usize; QUEUE_SIZE] = [0; QUEUE_SIZE];
static mut TX_BUFS: [usize; QUEUE_SIZE] = [0; QUEUE_SIZE];
static mut RX_BUF_PAGES: Vec<usize> = Vec::new();
static mut READY: bool = false;
static mut TX_SEQ: u32 = 0;

pub fn init() -> bool {
    let magic = reg32(MAGIC);
    if magic != 0x7472_6976 {
        crate::kprintln!("[virtio] bad magic {:#x}", magic);
        return false;
    }
    let version = reg32(VERSION);
    let devid = reg32(DEVICE_ID);
    crate::kprintln!(
        "[virtio] version={} device={} at {:#x}",
        version, devid, VIRTIO_BASE
    );
    if devid != 1 {
        crate::kprintln!("[virtio] not a network device");
        return false;
    }

    // reset
    wreg32(STATUS, 0);
    wreg32(STATUS, STATUS_ACK | STATUS_DRIVER);

    // negotiate features: VERSION_1 (bit 32), MAC (bit 5), STATUS (bit 16)
    wreg32(DEVICE_FEATURES_SEL, 1);
    let hi = reg32(DEVICE_FEATURES);
    wreg32(DEVICE_FEATURES_SEL, 0);
    let lo = reg32(DEVICE_FEATURES);
    let want_hi = 1u32; // VIRTIO_F_VERSION_1
    let want_lo = (1u32 << 5) | (1u32 << 16); // MAC | STATUS
    let offer_hi = hi & want_hi;
    let offer_lo = lo & want_lo;
    crate::kprintln!("[virtio] features lo={:#x} hi={:#x}", lo, hi);
    wreg32(DRIVER_FEATURES_SEL, 1);
    wreg32(DRIVER_FEATURES, offer_hi);
    wreg32(DRIVER_FEATURES_SEL, 0);
    wreg32(DRIVER_FEATURES, offer_lo);
    wreg32(STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
    if reg32(STATUS) & STATUS_FEATURES_OK == 0 {
        crate::kprintln!("[virtio] FEATURES_OK failed");
        return false;
    }

    // read MAC from config
    let mac: [u8; 6] = unsafe {
        core::ptr::read_volatile((VIRTIO_BASE + CONFIG) as *const [u8; 6])
    };
    crate::kprintln!("[virtio] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    unsafe {
        OUR_MAC = mac;
    }

    // set up queues
    let rq = setup_queue(0).expect("rx queue");
    let tq = setup_queue(1).expect("tx queue");
    unsafe {
        RX_QUEUE = Some(rq);
        TX_QUEUE = Some(tq);
    }

    wreg32(STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);

    // fill RX ring
    for i in 0..QUEUE_SIZE {
        give_rx_buf(i);
    }
    // notify device
    notify(0);

    unsafe {
        READY = true;
    }
    crate::kprintln!("[virtio] net ready");
    true
}

fn setup_queue(qidx: usize) -> Option<Vq> {
    wreg32(QUEUE_SEL, qidx as u32);
    let max = reg32(QUEUE_NUM_MAX);
    if max < QUEUE_SIZE as u32 {
        crate::kprintln!("[virtio] queue {} too small {}", qidx, max);
        return None;
    }
    wreg32(QUEUE_NUM, QUEUE_SIZE as u32);
    let desc = frame::alloc_frames(1)?;
    let avail = frame::alloc_frames(1)?;
    let used = frame::alloc_frames(1)?;
    unsafe {
        core::ptr::write_bytes(desc as *mut u8, 0, PAGE_SIZE);
        core::ptr::write_bytes(avail as *mut u8, 0, PAGE_SIZE);
        core::ptr::write_bytes(used as *mut u8, 0, PAGE_SIZE);
    }
    wreg64(QUEUE_DESC_LOW, desc as u64);
    wreg64(QUEUE_DESC_HIGH, 0);
    wreg64(QUEUE_DRIVER_LOW, avail as u64);
    wreg64(QUEUE_DRIVER_HIGH, 0);
    wreg64(QUEUE_DEVICE_LOW, used as u64);
    wreg64(QUEUE_DEVICE_HIGH, 0);
    wreg32(QUEUE_READY, 1);
    let vq = Vq {
        desc,
        avail,
        used,
        free: (0..QUEUE_SIZE).collect(),
        next_free: 0,
    };
    Some(vq)
}

fn notify(qidx: usize) {
    wreg32(QUEUE_NOTIFY, qidx as u32);
}

fn set_desc(desc: usize, idx: usize, addr: u64, len: u32, flags: u16, next: u16) {
    let base = desc + idx * 16;
    unsafe {
        let p = base as *mut u64;
        *p = addr;
        *p.add(1) = ((len as u64) << 32) | flags as u64;
        *p.add(1) |= (next as u64) << 48;
    }
}

/// Give the device an empty RX buffer for descriptor i.
fn give_rx_buf(i: usize) {
    let buf = frame::alloc_frame().expect("rx buf");
    unsafe {
        RX_BUFS[i] = buf;
        RX_BUF_PAGES.push(buf);
    }
    let vq = unsafe { RX_QUEUE.as_ref().unwrap() };
    set_desc(vq.desc, i, buf as u64, 2048, 0x0002 /* WRITE */, 0);
    // add to avail ring
    let avail = vq.avail;
    unsafe {
        let idx_ptr = (avail + 2) as *mut u16;
        let idx = idx_ptr.read_volatile() as usize;
        let ring = (avail + 4) as *mut u16;
        ring.add(idx % QUEUE_SIZE).write_volatile(i as u16);
        idx_ptr.write_volatile((idx + 1) as u16);
    }
}

/// Called from interrupt handler: process used buffers on both queues.
pub fn irq_handler() {
    let mut acked = false;
    unsafe {
        let status = reg32(INTERRUPT_STATUS);
        if status & 1 != 0 {
            acked = true;
            // RX
            if let Some(vq) = RX_QUEUE.as_ref() {
                let used = vq.used;
                let idx = ((used + 2) as *const u16).read_volatile() as usize;
                let last = ((used + 4) as *const u16).read_volatile() as usize;
                let mut processed = 0;
                while last != idx && processed < QUEUE_SIZE {
                    processed += 1;
                    let entry = (used + 6 + (last % QUEUE_SIZE) * 8) as *const u32;
                    let desc_id = entry.read_volatile() as usize;
                    let len = entry.add(1).read_volatile() as usize;
                    let buf = RX_BUFS[desc_id];
                    // virtio_net_hdr (10 bytes) precedes the frame
                    if len > VIRTIO_NET_HDR {
                        let frame = core::slice::from_raw_parts((buf + VIRTIO_NET_HDR) as *const u8, len - VIRTIO_NET_HDR);
                        crate::net::net_rx_frame(frame);
                    }
                    // hand buffer back
                    give_rx_buf(desc_id);
                    ((used + 4) as *mut u16).write_volatile((last + 1) as u16);
                    // update last
                    let _ = last;
                    // recompute last? we keep local counter
                    let _ = idx;
                    // break out of loop by updating stored last: store into used ring last slot
                    // We process one at a time: re-read
                    // To keep simple: process all entries up to idx by looping:
                    break;
                }
            }
            // TX: reclaim
            if let Some(vq) = TX_QUEUE.as_mut() {
                let used = vq.used;
                let idx = ((used + 2) as *const u16).read_volatile() as usize;
                let last = ((used + 4) as *const u16).read_volatile() as usize;
                let mut n = 0;
                while last != idx && n < QUEUE_SIZE {
                    n += 1;
                    let entry = (used + 6 + (last % QUEUE_SIZE) * 8) as *const u32;
                    let desc_id = entry.read_volatile() as usize;
                    let buf = TX_BUFS[desc_id];
                    if buf != 0 {
                        frame::free_frame(buf);
                        TX_BUFS[desc_id] = 0;
                        vq.free.push(desc_id);
                    }
                    ((used + 4) as *mut u16).write_volatile((last + 1) as u16);
                }
            }
            // ACK the interrupt
            wreg32(INTERRUPT_ACK, 3);
        }
    }
    let _ = acked;
    let _ = READY;
}

/// Transmit one ethernet frame.
pub fn net_tx(frame: &[u8]) {
    if !unsafe { READY } {
        return;
    }
    // get a free TX descriptor
    let (desc_id, desc, avail, buf) = unsafe {
        let vq = TX_QUEUE.as_mut().unwrap();
        let desc_id = match vq.free.pop() {
            Some(d) => d,
            None => return, // no free desc; drop (caller retries on RTO)
        };
        let buf = frame::alloc_frame().expect("tx buf");
        TX_BUFS[desc_id] = buf;
        (desc_id, vq.desc, vq.avail, buf)
    };
    // header (10 zero bytes) + frame
    unsafe {
        let p = buf as *mut u8;
        for i in 0..VIRTIO_NET_HDR {
            *p.add(i) = 0;
        }
        core::ptr::copy_nonoverlapping(frame.as_ptr(), p.add(VIRTIO_NET_HDR), frame.len());
    }
    let total = VIRTIO_NET_HDR + frame.len();
    set_desc(desc, desc_id, buf as u64, total as u32, 0, 0);
    // avail ring
    unsafe {
        let idx_ptr = (avail + 2) as *mut u16;
        let idx = idx_ptr.read_volatile() as usize;
        let ring = (avail + 4) as *mut u16;
        ring.add(idx % QUEUE_SIZE).write_volatile(desc_id as u16);
        idx_ptr.write_volatile((idx + 1) as u16);
    }
    notify(1);
}

pub fn tx_pending() -> usize {
    unsafe { TX_QUEUE.as_ref().map(|q| q.free.len()).unwrap_or(0) }
}
