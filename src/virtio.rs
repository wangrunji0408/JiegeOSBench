// Legacy virtio-mmio network device driver (polling, no interrupts).
use core::sync::atomic::{fence, Ordering};

pub const NUM: usize = 64;
const BSIZE: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(4096))]
struct Ring {
    b: [u8; 8192],
}

static mut RXR: Ring = Ring { b: [0; 8192] };
static mut TXR: Ring = Ring { b: [0; 8192] };
static mut RXB: [[u8; BSIZE]; NUM] = [[0; BSIZE]; NUM];
static mut TXB: [[u8; BSIZE]; NUM] = [[0; BSIZE]; NUM];

static mut BASE: usize = 0;
static mut RX_LAST: u16 = 0;
static mut RX_AVAIL: u16 = 0;
static mut TX_HEAD: u16 = 0;
static mut TX_TAIL: u16 = 0;
static mut TX_AVAIL: u16 = 0;
static mut TX_USED_LAST: u16 = 0;

pub static mut MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
pub static mut RX_COUNT: u64 = 0;
pub static mut TX_COUNT: u64 = 0;

fn rd(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((BASE + off) as *const u32) }
}
fn wr(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((BASE + off) as *mut u32, v) }
}

pub fn present() -> bool {
    unsafe { BASE != 0 }
}

pub fn init() -> [u8; 6] {
    unsafe {
        for i in 0..8 {
            let b = 0x1000_1000 + i * 0x1000;
            let magic = core::ptr::read_volatile((b + 0x000) as *const u32);
            let ver = core::ptr::read_volatile((b + 0x004) as *const u32);
            let dev = core::ptr::read_volatile((b + 0x008) as *const u32);
            if magic == 0x7472_6976 && dev == 1 {
                if ver != 1 {
                    crate::println!("[virtio] unsupported version {}", ver);
                    continue;
                }
                BASE = b;
                crate::println!("[virtio] net device at {:#x}", b);
                break;
            }
        }
        if BASE == 0 {
            return MAC;
        }
        // reset
        wr(0x70, 0);
        wr(0x70, 1); // ACKNOWLEDGE
        wr(0x70, 3); // ACKNOWLEDGE | DRIVER
        wr(0x28, 4096); // guest page size

        let host = rd(0x10);
        let mut guest = 0u32;
        if host & (1 << 5) != 0 {
            guest |= 1 << 5; // VIRTIO_NET_F_MAC
        }
        wr(0x20, guest);

        // RX queue
        let rxaddr = (&raw const RXR) as usize;
        wr(0x30, 0);
        let max = rd(0x34);
        let num = (max as usize).min(NUM).max(2);
        wr(0x38, num as u32);
        wr(0x3c, 4096);
        let desc = rxaddr as *mut Desc;
        for i in 0..num {
            (*desc.add(i)).addr = (&raw const RXB[i]) as u64;
            (*desc.add(i)).len = BSIZE as u32;
            (*desc.add(i)).flags = 2; // VRING_DESC_F_WRITE
            (*desc.add(i)).next = 0;
        }
        let avail = (rxaddr + num * 16) as *mut u16;
        *avail = 0;
        *avail.add(1) = num as u16;
        for i in 0..num {
            *avail.add(2 + i) = i as u16;
        }
        RX_AVAIL = num as u16;
        let used = (rxaddr + 4096) as *mut u16;
        *used = 0;
        *used.add(1) = 0;
        wr(0x40, (rxaddr >> 12) as u32);

        // TX queue
        let txaddr = (&raw const TXR) as usize;
        wr(0x30, 1);
        let max2 = rd(0x34);
        let num2 = (max2 as usize).min(NUM).max(2);
        wr(0x38, num2 as u32);
        wr(0x3c, 4096);
        let desc2 = txaddr as *mut Desc;
        for i in 0..num2 {
            (*desc2.add(i)).addr = (&raw const TXB[i]) as u64;
            (*desc2.add(i)).len = 0;
            (*desc2.add(i)).flags = 0;
            (*desc2.add(i)).next = 0;
        }
        let avail2 = (txaddr + num2 * 16) as *mut u16;
        *avail2 = 0;
        *avail2.add(1) = 0;
        TX_AVAIL = 0;
        let used2 = (txaddr + 4096) as *mut u16;
        *used2 = 0;
        *used2.add(1) = 0;
        wr(0x40, (txaddr >> 12) as u32);

        wr(0x70, 7); // DRIVER_OK
        fence(Ordering::SeqCst);
        wr(0x50, 0); // notify RX queue: descriptors available


        if guest & (1 << 5) != 0 {
            let p = (BASE + 0x100) as *const u8;
            for i in 0..6 {
                MAC[i] = core::ptr::read_volatile(p.add(i));
            }
        }
        crate::println!(
            "[virtio] mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            MAC[0],
            MAC[1],
            MAC[2],
            MAC[3],
            MAC[4],
            MAC[5]
        );
        MAC
    }
}

/// Pop a received packet: returns (descriptor id, pointer, length).
pub fn rx_pop() -> Option<(usize, usize, usize)> {
    unsafe {
        if BASE == 0 {
            return None;
        }
        fence(Ordering::SeqCst);
        let mem = (&raw const RXR) as usize;
        let used_idx = core::ptr::read_volatile((mem + 4096 + 2) as *const u16);
        if used_idx == RX_LAST {
            return None;
        }
        let slot = (RX_LAST as usize) % NUM;
        let ent = (mem + 4096 + 4 + slot * 8) as *const u32;
        let id = core::ptr::read_volatile(ent) as usize;
        let len = core::ptr::read_volatile(ent.add(1)) as usize;
        RX_LAST = RX_LAST.wrapping_add(1);
        RX_COUNT += 1;
        if id >= NUM || len > BSIZE {
            return None;
        }
        Some((id, (&raw const RXB[id]) as usize, len))
    }
}

pub fn rx_release(id: usize) {
    unsafe {
        if BASE == 0 {
            return;
        }
        let mem = (&raw const RXR) as usize;
        let avail = (mem + NUM * 16) as *mut u16;
        let ring = avail.add(2);
        *ring.add((RX_AVAIL as usize) % NUM) = id as u16;
        RX_AVAIL = RX_AVAIL.wrapping_add(1);
        *avail.add(1) = RX_AVAIL;
        fence(Ordering::SeqCst);
        wr(0x50, 0);
    }
}

fn reclaim_tx() {
    unsafe {
        if BASE == 0 {
            return;
        }
        fence(Ordering::SeqCst);
        let mem = (&raw const TXR) as usize;
        let used_idx = core::ptr::read_volatile((mem + 4096 + 2) as *const u16);
        TX_TAIL = used_idx;
        let _ = TX_USED_LAST;
    }
}

pub fn tx_can_send() -> bool {
    reclaim_tx();
    unsafe { TX_HEAD.wrapping_sub(TX_TAIL) < NUM as u16 }
}

pub fn tx_send<R>(len: usize, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
    if len > BSIZE {
        return None;
    }
    reclaim_tx();
    unsafe {
        if TX_HEAD.wrapping_sub(TX_TAIL) >= NUM as u16 {
            return None;
        }
        let id = (TX_HEAD as usize) % NUM;
        let buf = &mut TXB[id][..len];
        let out = f(buf);
        let mem = (&raw const TXR) as usize;
        let desc = mem as *mut Desc;
        (*desc.add(id)).addr = (&raw const TXB[id]) as u64;
        (*desc.add(id)).len = len as u32;
        (*desc.add(id)).flags = 0;
        (*desc.add(id)).next = 0;
        let avail = (mem + NUM * 16) as *mut u16;
        let ring = avail.add(2);
        *ring.add((TX_AVAIL as usize) % NUM) = id as u16;
        TX_AVAIL = TX_AVAIL.wrapping_add(1);
        *avail.add(1) = TX_AVAIL;
        TX_HEAD = TX_HEAD.wrapping_add(1);
        TX_COUNT += 1;
        fence(Ordering::SeqCst);
        wr(0x50, 1);
        Some(out)
    }
}

pub fn debug_dump() {
    unsafe {
        if BASE == 0 {
            return;
        }
        wr(0x30, 0);
        let num = rd(0x38);
        let pfn = rd(0x40);
        let mem = (&raw const RXR) as usize;
        let avail_idx = core::ptr::read_volatile((mem + NUM * 16 + 2) as *const u16);
        let used_idx = core::ptr::read_volatile((mem + 4096 + 2) as *const u16);
        let status = rd(0x70);
        let isr = rd(0x60);
        let d0 = core::ptr::read_volatile(mem as *const u64);
        let d0len = core::ptr::read_volatile((mem + 8) as *const u32);
        crate::println!(
            "[virtio] status={} num={} pfn={:#x} avail_idx={} used_idx={} isr={:#x} desc0={:#x}/{}",
            status, num, pfn, avail_idx, used_idx, isr, d0, d0len
        );
    }
}
