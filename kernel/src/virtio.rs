//! virtio network driver for QEMU virt.
//! Supports both legacy (version 1) and modern (version 2) virtio-mmio.

use alloc::vec::Vec


use crate::mm::frame

use crate::net::OUR_MAC


const QUEUE_SIZE: usize = 64

const VIRTIO_NET_HDR: usize = 10


// feature bits (same numbering in legacy and modern)
const F_CSUM: u32 = 1 << 0

const F_MAC: u32 = 1 << 5

const F_STATUS: u32 = 1 << 16


const STATUS_ACK: u32 = 1

const STATUS_DRIVER: u32 = 2

const STATUS_DRIVER_OK: u32 = 4

const STATUS_FEATURES_OK: u32 = 8


// modern register offsets
const M_MAGIC: usize = 0x000

const M_VERSION: usize = 0x004

const M_DEVICE_ID: usize = 0x008

const M_DEVICE_FEATURES: usize = 0x010

const M_DEVICE_FEATURES_SEL: usize = 0x014

const M_DRIVER_FEATURES: usize = 0x020

const M_DRIVER_FEATURES_SEL: usize = 0x024

const M_QUEUE_SEL: usize = 0x028

const M_QUEUE_NUM_MAX: usize = 0x02c

const M_QUEUE_NUM: usize = 0x030

const M_QUEUE_READY: usize = 0x034

const M_QUEUE_DESC_LOW: usize = 0x044

const M_QUEUE_DESC_HIGH: usize = 0x048

const M_QUEUE_DRIVER_LOW: usize = 0x050

const M_QUEUE_DRIVER_HIGH: usize = 0x054

const M_QUEUE_DEVICE_LOW: usize = 0x058

const M_QUEUE_DEVICE_HIGH: usize = 0x05c

const M_QUEUE_NOTIFY: usize = 0x060

const M_INTERRUPT_STATUS: usize = 0x064

const M_INTERRUPT_ACK: usize = 0x068

const M_STATUS: usize = 0x070

const M_CONFIG: usize = 0x100


// legacy register offsets
const L_QUEUE_NUM: usize = 0x030

const L_QUEUE_PFN: usize = 0x034

const L_QUEUE_NOTIFY: usize = 0x03c

const L_INTERRUPT_STATUS: usize = 0x050

const L_INTERRUPT_ACK: usize = 0x054

const L_STATUS: usize = 0x060

const L_CONFIG: usize = 0x070


static mut DEV_BASE: usize = 0

static mut LEGACY: bool = false


struct Vq {
    desc: usize,
    avail: usize,
    used: usize,
    free: Vec<usize>,
}

static mut RX_QUEUE: Option<Vq> = None

static mut TX_QUEUE: Option<Vq> = None

static mut RX_BUFS: [usize
 QUEUE_SIZE] = [0
 QUEUE_SIZE]

static mut TX_BUFS: [usize
 QUEUE_SIZE] = [0
 QUEUE_SIZE]

static mut READY: bool = false


fn reg32(off: usize) -> u32 {
    unsafe { ((DEV_BASE + off) as *const u32).read_volatile() }
}
fn wreg32(off: usize, v: u32) {
    unsafe {
        ((DEV_BASE + off) as *mut u32).write_volatile(v)

    }
}
fn reg64(off: usize) -> u64 {
    unsafe { ((DEV_BASE + off) as *const u64).read_volatile() }
}
fn wreg64(off: usize, v: u64) {
    unsafe {
        ((DEV_BASE + off) as *mut u64).write_volatile(v)

    }
}

fn find_device() -> usize {
    for i in 0..32usize {
        let base = 0x1000_0000 + i * 0x1000

        let magic = unsafe { ((base + M_MAGIC) as *const u32).read_volatile() }

        if magic != 0x7472_6976 {
            continue

        }
        let version = unsafe { ((base + M_VERSION) as *const u32).read_volatile() }

        let devid = unsafe { ((base + M_DEVICE_ID) as *const u32).read_volatile() }

        if devid == 1 {
            return base

        }
        crate::kprintln!("[virtio] slot {:#x}: magic ok version={} device={}", base, version, devid)

    }
    0
}

pub fn init() -> bool {
    let base = find_device()

    if base == 0 {
        crate::kprintln!("[virtio] no network device found")

        return false

    }
    unsafe {
        DEV_BASE = base

    }
    let version = reg32(M_VERSION)

    let devid = reg32(M_DEVICE_ID)

    crate::kprintln!("[virtio] network device at {:#x}, version={}, id={}", base, version, devid)

    if version == 1 {
        unsafe {
            LEGACY = true

        }
        crate::kprintln!("[virtio] legacy mode")

    }

    // reset
    wreg32(M_STATUS, 0)

    wreg32(M_STATUS, STATUS_ACK | STATUS_DRIVER)


    // negotiate features: CSUM | MAC | STATUS (and VERSION_1 if modern)
    if unsafe { LEGACY } {
        let host = reg32(M_DEVICE_FEATURES)

        crate::kprintln!("[virtio] host features {:#x}", host)

        let want = F_CSUM | F_MAC | F_STATUS

        let offer = host & want

        wreg32(M_DRIVER_FEATURES, offer)

        crate::kprintln!("[virtio] offered features {:#x}", offer)

    } else {
        wreg32(M_DEVICE_FEATURES_SEL, 1)

        let hi = reg32(M_DEVICE_FEATURES)

        wreg32(M_DEVICE_FEATURES_SEL, 0)

        let lo = reg32(M_DEVICE_FEATURES)

        crate::kprintln!("[virtio] host features lo={:#x} hi={:#x}", lo, hi)

        let offer_hi = hi & 1
 // VERSION_1
        let offer_lo = lo & (F_CSUM | F_MAC | F_STATUS)

        wreg32(M_DRIVER_FEATURES_SEL, 1)

        wreg32(M_DRIVER_FEATURES, offer_hi)

        wreg32(M_DRIVER_FEATURES_SEL, 0)

        wreg32(M_DRIVER_FEATURES, offer_lo)

        wreg32(M_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK)

        if reg32(M_STATUS) & STATUS_FEATURES_OK == 0 {
            crate::kprintln!("[virtio] FEATURES_OK failed")

            return false

        }
    }

    // read MAC
    let cfg_off = if unsafe { LEGACY } { L_CONFIG } else { M_CONFIG }

    let mac: [u8
 6] = unsafe { core::ptr::read_volatile((DEV_BASE + cfg_off) as *const [u8
 6]) }

    crate::kprintln!(
        "[virtio] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )

    unsafe {
        OUR_MAC = mac

    }

    // queues
    let rq = setup_queue(0).expect("rx queue")

    let tq = setup_queue(1).expect("tx queue")

    unsafe {
        RX_QUEUE = Some(rq)

        TX_QUEUE = Some(tq)

    }

    wreg32(M_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_DRIVER_OK)

    if !unsafe { LEGACY } {
        wreg32(M_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK)

    }

    // fill RX ring
    for i in 0..QUEUE_SIZE {
        give_rx_buf(i)

    }
    notify(0)


    unsafe {
        READY = true

    }
    crate::kprintln!("[virtio] net ready")

    true
}

fn setup_queue(qidx: usize) -> Option<Vq> {
    wreg32(M_QUEUE_SEL, qidx as u32)

    if unsafe { LEGACY } {
        // legacy: queue is 2 contiguous pages: desc @0, avail @0x800, used @0x1000
        let pages = frame::alloc_frames(2)?

        unsafe {
            core::ptr::write_bytes(pages as *mut u8, 0, 2 * 4096)

        }
        wreg32(L_QUEUE_NUM, QUEUE_SIZE as u32)

        wreg32(L_QUEUE_PFN, (pages >> 12) as u32)

        wreg32(0x038, 4096)
 // QueueAlign
        crate::kprintln!("[virtio] queue {} legacy at {:#x}", qidx, pages)

        Some(Vq {
            desc: pages,
            avail: pages + 0x800,
            used: pages + 0x1000,
            free: (0..QUEUE_SIZE).collect(),
        })
    } else {
        let max = reg32(M_QUEUE_NUM_MAX)

        crate::kprintln!("[virtio] queue {} max={}", qidx, max)

        if max < QUEUE_SIZE as u32 {
            return None

        }
        wreg32(M_QUEUE_NUM, QUEUE_SIZE as u32)

        let desc = frame::alloc_frames(1)?

        let avail = frame::alloc_frames(1)?

        let used = frame::alloc_frames(1)?

        unsafe {
            core::ptr::write_bytes(desc as *mut u8, 0, 4096)

            core::ptr::write_bytes(avail as *mut u8, 0, 4096)

            core::ptr::write_bytes(used as *mut u8, 0, 4096)

        }
        wreg64(M_QUEUE_DESC_LOW, desc as u64)

        wreg64(M_QUEUE_DESC_HIGH, 0)

        wreg64(M_QUEUE_DRIVER_LOW, avail as u64)

        wreg64(M_QUEUE_DRIVER_HIGH, 0)

        wreg64(M_QUEUE_DEVICE_LOW, used as u64)

        wreg64(M_QUEUE_DEVICE_HIGH, 0)

        wreg32(M_QUEUE_READY, 1)

        Some(Vq {
            desc,
            avail,
            used,
            free: (0..QUEUE_SIZE).collect(),
        })
    }
}

fn notify(qidx: usize) {
    if unsafe { LEGACY } {
        wreg32(L_QUEUE_NOTIFY, qidx as u32)

    } else {
        wreg32(M_QUEUE_NOTIFY, qidx as u32)

    }
}

fn set_desc(vq: &Vq, idx: usize, addr: u64, len: u32, flags: u16, next: u16) {
    let base = vq.desc + idx * 16

    unsafe {
        let p = base as *mut u64

        *p = addr

        *p.add(1) = ((len as u64) << 32) | flags as u64 | ((next as u64) << 48)

    }
}

fn give_rx_buf(i: usize) {
    let buf = frame::alloc_frame().expect("rx buf")

    unsafe {
        RX_BUFS[i] = buf

    }
    let vq = unsafe { RX_QUEUE.as_ref().unwrap() }

    set_desc(vq, i, buf as u64, 2048, 0x0002, 0)

    unsafe {
        let idx_ptr = (vq.avail + 2) as *mut u16

        let idx = idx_ptr.read_volatile() as usize

        let ring = (vq.avail + 4) as *mut u16

        ring.add(idx % QUEUE_SIZE).write_volatile(i as u16)

        idx_ptr.write_volatile((idx + 1) as u16)

    }
}

pub fn irq_handler() {
    let (istat_off, iack_off) = if unsafe { LEGACY } {
        (L_INTERRUPT_STATUS, L_INTERRUPT_ACK)
    } else {
        (M_INTERRUPT_STATUS, M_INTERRUPT_ACK)
    }

    unsafe {
        let status = reg32(istat_off)

        if status & 1 != 0 {
            // RX
            if let Some(vq) = RX_QUEUE.as_ref() {
                let used = vq.used

                let idx = ((used + 2) as *const u16).read_volatile() as usize

                let mut last = ((used + 4) as *const u16).read_volatile() as usize

                let mut n = 0

                while last != idx 
 n < QUEUE_SIZE {
                    n += 1

                    let entry = (used + 4 + (last % QUEUE_SIZE) * 8) as *const u32

                    let desc_id = entry.read_volatile() as usize

                    let len = entry.add(1).read_volatile() as usize

                    let buf = RX_BUFS[desc_id]

                    if len > VIRTIO_NET_HDR {
                        let frame = core::slice::from_raw_parts(
                            (buf + VIRTIO_NET_HDR) as *const u8,
                            len - VIRTIO_NET_HDR,
                        )

                        crate::net::net_rx_frame(frame)

                    }
                    give_rx_buf(desc_id)

                    last += 1

                }
                ((used + 4) as *mut u16).write_volatile(last as u16)

            }
            // TX
            if let Some(vq) = TX_QUEUE.as_mut() {
                let used = vq.used

                let idx = ((used + 2) as *const u16).read_volatile() as usize

                let mut last = ((used + 4) as *const u16).read_volatile() as usize

                let mut n = 0

                while last != idx 
 n < QUEUE_SIZE {
                    n += 1

                    let entry = (used + 4 + (last % QUEUE_SIZE) * 8) as *const u32

                    let desc_id = entry.read_volatile() as usize

                    let buf = TX_BUFS[desc_id]

                    if buf != 0 {
                        frame::free_frame(buf)

                        TX_BUFS[desc_id] = 0

                        vq.free.push(desc_id)

                    }
                    last += 1

                }
                ((used + 4) as *mut u16).write_volatile(last as u16)

            }
            wreg32(iack_off, 1)

        }
        if status & 2 != 0 {
            wreg32(iack_off, 2)

        }
    }
}

pub fn net_tx(frame: &[u8]) {
    if !unsafe { READY } {
        return

    }
    let (desc_id, desc, avail, buf) = unsafe {
        let vq = TX_QUEUE.as_mut().unwrap()

        let desc_id = match vq.free.pop() {
            Some(d) => d,
            None => return,
        }

        let buf = frame::alloc_frame().expect("tx buf")

        TX_BUFS[desc_id] = buf

        (desc_id, vq.desc, vq.avail, buf)
    }

    unsafe {
        let p = buf as *mut u8

        for i in 0..VIRTIO_NET_HDR {
            *p.add(i) = 0

        }
        core::ptr::copy_nonoverlapping(frame.as_ptr(), p.add(VIRTIO_NET_HDR), frame.len())

    }
    let total = VIRTIO_NET_HDR + frame.len()

    let vq = Vq {
        desc,
        avail,
        used: 0,
        free: Vec::new(),
    }

    set_desc(&vq, desc_id, buf as u64, total as u32, 0, 0)

    unsafe {
        let idx_ptr = (avail + 2) as *mut u16

        let idx = idx_ptr.read_volatile() as usize

        let ring = (avail + 4) as *mut u16

        ring.add(idx % QUEUE_SIZE).write_volatile(desc_id as u16)

        idx_ptr.write_volatile((idx + 1) as u16)

    }
    notify(1)

}
