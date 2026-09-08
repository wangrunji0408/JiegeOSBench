//! virtio-net driver for the QEMU `virt` machine.
//!
//! Supports **both** virtio-mmio transports, because the reference QEMU launch
//! line (`-device virtio-net-device,netdev=n0`, no `-global
//! virtio-mmio.force-legacy=off`) exposes the device as **version 1 (legacy)**:
//!
//! * version 2 (modern, virtio 1.x): 64-bit queue addresses via
//!   QueueDesc/Driver/DeviceLow+High, `QueueReady`, `FEATURES_OK`, and
//!   `VIRTIO_F_VERSION_1` negotiated.
//! * version 1 (legacy): `GuestPageSize`/`QueueAlign`/`QueuePFN`, single 32-bit
//!   feature word, no `FEATURES_OK`.
//!
//! The transport matters for more than register layout: it selects the size of
//! the per-packet virtio-net header (see [`HDR_LEGACY`] / [`HDR_MODERN`]).
//!
//! All MMIO access goes through `crate::mm::address::dev_addr` because the
//! device region is *not* identity mapped in the kernel page table.

use crate::mm::address::dev_addr;
use crate::mm::frame::{alloc_frames, free_frames};
use core::sync::atomic::{fence, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// MMIO register offsets.  Modern layout: virtio 1.1 section 4.2.2.  Legacy
// layout: virtio 1.1 section 4.2.4, cross-checked against
// include/uapi/linux/virtio_mmio.h.
// ---------------------------------------------------------------------------
const R_MAGIC: usize = 0x000; // "virt" = 0x74726976
const R_VERSION: usize = 0x004; // 1 = legacy, 2 = modern
const R_DEVICE_ID: usize = 0x008; // 1 = network device
const R_VENDOR_ID: usize = 0x00c; // 0x554d4551 ("QEMU")
const R_DEVICE_FEATURES: usize = 0x010;
const R_DEVICE_FEATURES_SEL: usize = 0x014; // modern only
const R_DRIVER_FEATURES: usize = 0x020;
const R_DRIVER_FEATURES_SEL: usize = 0x024; // modern only
const R_GUEST_PAGE_SIZE: usize = 0x028; // legacy only
const R_QUEUE_SEL: usize = 0x030;
const R_QUEUE_NUM_MAX: usize = 0x034;
const R_QUEUE_NUM: usize = 0x038;
const R_QUEUE_ALIGN: usize = 0x03c; // legacy only
const R_QUEUE_PFN: usize = 0x040; // legacy only
const R_QUEUE_READY: usize = 0x044; // modern only
const R_QUEUE_NOTIFY: usize = 0x050;
const R_INTERRUPT_STATUS: usize = 0x060;
const R_INTERRUPT_ACK: usize = 0x064;
const R_STATUS: usize = 0x070;
const R_QUEUE_DESC_LOW: usize = 0x080; // modern
const R_QUEUE_DESC_HIGH: usize = 0x084;
const R_QUEUE_DRIVER_LOW: usize = 0x090; // modern, available ring
const R_QUEUE_DRIVER_HIGH: usize = 0x094;
const R_QUEUE_DEVICE_LOW: usize = 0x0a0; // modern, used ring
const R_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const R_CONFIG_GENERATION: usize = 0x0fc;
const R_CONFIG: usize = 0x100; // device-specific config space (MAC)

const MAGIC_VALUE: u32 = 0x7472_6976;
const VENDOR_QEMU: u32 = 0x554d_4551;
const DEVICE_ID_NET: u32 = 1;

// Device status bits (virtio 1.1 section 2.1).
const ST_ACKNOWLEDGE: u32 = 1;
const ST_DRIVER: u32 = 2;
const ST_DRIVER_OK: u32 = 4;
const ST_FEATURES_OK: u32 = 8;

/// `VIRTIO_NET_F_MAC` (bit 5): device config space holds our MAC address.
const FEAT_MAC: u32 = 1 << 5;
/// `VIRTIO_F_VERSION_1` (bit 32) = feature word 1, bit 0.  Modern transport only.
const FEAT_VERSION_1_WORD1: u32 = 1 << 0;

/// Descriptor flag: buffer is device-writable (RX).
const VIRTQ_DESC_F_WRITE: u16 = 0x2;

/// Legacy virtqueue alignment / guest page size, in bytes.
const LEGACY_ALIGN: usize = 4096;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Descriptors per virtqueue.  QEMU reports QueueNumMax = 256 for virtio-net.
pub const QUEUE_NUM: u16 = 128;
/// Bytes per RX/TX buffer.  Must hold the virtio-net header plus a full
/// 1514-byte Ethernet frame in a *single* buffer, because we do not negotiate
/// `VIRTIO_NET_F_MRG_RXBUF` (QEMU drops packets that do not fit in one buffer).
pub const BUF_SIZE: usize = 2048;

/// virtio-net header size on a **legacy** transport without `VIRTIO_NET_F_MRG_RXBUF`:
/// `struct virtio_net_hdr` = flags, gso_type, hdr_len, gso_size, csum_start,
/// csum_offset = 10 bytes.
pub const HDR_LEGACY: usize = 10;

/// virtio-net header size on a **modern** transport: `struct virtio_net_hdr_v1`
/// = the 10 legacy bytes plus `num_buffers` = 12 bytes.
///
/// This is 12 bytes even though `VIRTIO_NET_F_MRG_RXBUF` is *not* negotiated,
/// because `VIRTIO_F_VERSION_1` *is*: QEMU's `virtio_net_set_mrg_rx_bufs()`
/// sets `guest_hdr_len = sizeof(struct virtio_net_hdr_mrg_rxbuf)` whenever
/// version 1 is negotiated.  Using 10 there corrupts every packet, and using
/// 12 on a legacy device does the same in the other direction.
pub const HDR_MODERN: usize = 12;

/// virtio-mmio slots on the QEMU virt machine: 0x1000_1000 + 0x1000 * slot.
pub const SLOT_BASE: usize = 0x1000_1000;
pub const SLOT_STRIDE: usize = 0x1000;
pub const MAX_SLOTS: usize = 8;

/// MMIO base of the probed device, for the interrupt handler's InterruptACK.
/// Zero means "no device"; the handler ACKs without taking any lock, otherwise
/// a level-triggered PLIC interrupt would storm while a thread holds the lock.
static IRQ_BASE: AtomicUsize = AtomicUsize::new(0);

/// Which MMIO transport the device speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    /// MMIO version 1: GuestPageSize/QueueAlign/QueuePFN.
    Legacy,
    /// MMIO version 2: 64-bit queue addresses, QueueReady, FEATURES_OK.
    Modern,
}

// ---------------------------------------------------------------------------
// Split virtqueue structures (virtio 1.1 section 2.6)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

const _: () = assert!(core::mem::size_of::<Desc>() == 16);
const _: () = assert!(core::mem::size_of::<UsedElem>() == 8);

#[inline]
fn r32(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(dev_addr(base + off) as *const u32) }
}

#[inline]
fn w32(base: usize, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile(dev_addr(base + off) as *mut u32, v) }
}

#[inline]
fn r8(base: usize, off: usize) -> u8 {
    unsafe { core::ptr::read_volatile(dev_addr(base + off) as *const u8) }
}

#[inline]
fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

/// One split virtqueue: descriptor table + available ring + used ring + buffers.
///
/// The ring parts and the buffers live in frame-allocated memory that is both
/// identity mapped and physically contiguous, so every pointer stored here is
/// simultaneously a kernel VA and the physical address handed to the device.
pub struct Vq {
    base: usize,
    qidx: u32,
    num: u16,
    /// virtio-net header size for this transport ([`HDR_LEGACY`]/[`HDR_MODERN`]).
    hdr_len: usize,
    desc: *mut Desc,
    avail_idx: *mut u16,
    avail_ring: *mut u16,
    used_idx: *mut u16,
    used_ring: *mut UsedElem,
    buf_pa: usize,
    buf_va: usize,
    ring_pa: usize,
    ring_frames: usize,
    buf_frames: usize,
}

// Single hart; the whole `Net` value lives behind one SpinLock.
unsafe impl Send for Vq {}

impl Vq {
    #[inline]
    fn num(&self) -> usize {
        self.num as usize
    }

    #[inline]
    fn notify(&self) {
        fence(Ordering::SeqCst);
        w32(self.base, R_QUEUE_NOTIFY, self.qidx);
    }

    /// Publish descriptor `id` on the available ring and notify the device.
    #[inline]
    fn add_avail(&self, id: u16) {
        unsafe {
            let ai = core::ptr::read_volatile(self.avail_idx);
            core::ptr::write_volatile(self.avail_ring.add((ai as usize) % self.num()), id);
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(self.avail_idx, ai.wrapping_add(1));
        }
        self.notify();
    }

    #[inline]
    fn buf_va(&self, id: u16) -> usize {
        self.buf_va + id as usize * BUF_SIZE
    }

    #[inline]
    fn buf_pa(&self, id: u16) -> usize {
        self.buf_pa + id as usize * BUF_SIZE
    }
}

/// RX half of the driver.
pub struct RxQueue {
    q: Vq,
    last_used: u16,
    /// Frames handed to smoltcp.
    pub packets: u64,
    /// Used-ring entries rejected as malformed.
    pub malformed: u64,
}

/// TX half of the driver.
pub struct TxQueue {
    q: Vq,
    last_used: u16,
    free_head: u16,
    num_free: u16,
    /// Packets submitted to the device.
    pub packets: u64,
    /// Packets dropped because the ring was full or the frame was too large.
    pub dropped: u64,
}

impl RxQueue {
    /// Take one completed RX buffer.  Returns `(descriptor id, frame length)`
    /// where the frame starts at `hdr_len` inside that buffer.
    pub(crate) fn pop_used(&mut self) -> Option<(u16, usize)> {
        loop {
            let uidx = unsafe { core::ptr::read_volatile(self.q.used_idx) };
            if self.last_used == uidx {
                return None;
            }
            fence(Ordering::Acquire);
            let slot = (self.last_used as usize) % self.q.num();
            let e = unsafe { core::ptr::read_volatile(self.q.used_ring.add(slot)) };
            self.last_used = self.last_used.wrapping_add(1);

            let id = e.id as usize;
            let len = e.len as usize;
            if id < self.q.num() {
                if len >= self.q.hdr_len && len <= BUF_SIZE {
                    return Some((id as u16, len - self.q.hdr_len));
                }
                // Malformed length: give the buffer back and keep going.
                self.malformed += 1;
                self.recycle(id as u16);
            } else {
                self.malformed += 1;
            }
        }
    }

    /// Bytes of the received frame (virtio-net header already skipped).
    ///
    /// `&mut` because `smoltcp::phy::RxToken::consume` hands out a mutable
    /// slice; smoltcp only reads it.
    pub(crate) fn frame_mut(&mut self, id: u16, len: usize) -> &mut [u8] {
        let p = (self.q.buf_va(id) + self.q.hdr_len) as *mut u8;
        unsafe { core::slice::from_raw_parts_mut(p, len) }
    }

    /// Put a buffer back on the available ring.
    pub(crate) fn recycle(&mut self, id: u16) {
        unsafe {
            let d = self.q.desc.add(id as usize);
            (*d).addr = self.q.buf_pa(id) as u64;
            (*d).len = BUF_SIZE as u32;
            (*d).flags = VIRTQ_DESC_F_WRITE;
            (*d).next = 0;
        }
        self.q.add_avail(id);
    }

    pub fn used_idx(&self) -> u16 {
        unsafe { core::ptr::read_volatile(self.q.used_idx) }
    }

    pub fn last_used(&self) -> u16 {
        self.last_used
    }

    pub fn num(&self) -> u16 {
        self.q.num
    }
}

impl TxQueue {
    /// Reclaim descriptors the device has finished with.
    pub(crate) fn reclaim(&mut self) {
        loop {
            let uidx = unsafe { core::ptr::read_volatile(self.q.used_idx) };
            if self.last_used == uidx {
                return;
            }
            fence(Ordering::Acquire);
            let slot = (self.last_used as usize) % self.q.num();
            let e = unsafe { core::ptr::read_volatile(self.q.used_ring.add(slot)) };
            self.last_used = self.last_used.wrapping_add(1);
            let id = e.id as usize;
            if id < self.q.num() {
                unsafe {
                    let d = self.q.desc.add(id);
                    (*d).next = self.free_head;
                }
                self.free_head = id as u16;
                self.num_free += 1;
            }
        }
    }

    #[inline]
    pub fn free(&self) -> u16 {
        self.num_free
    }

    pub fn used_idx(&self) -> u16 {
        unsafe { core::ptr::read_volatile(self.q.used_idx) }
    }

    pub fn last_used(&self) -> u16 {
        self.last_used
    }

    pub fn num(&self) -> u16 {
        self.q.num
    }

    /// Build one packet of `len` frame bytes with the closure and submit it.
    ///
    /// If the ring is full (or the frame cannot fit in one buffer) the closure
    /// is still called, on a scratch buffer, and the packet is dropped; callers
    /// that can retry should check [`TxQueue::free`] first.
    pub fn send<R>(&mut self, len: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
        self.reclaim();
        let hdr = self.q.hdr_len;
        if self.num_free == 0 || hdr + len > BUF_SIZE {
            self.dropped += 1;
            let mut scratch = [0u8; BUF_SIZE];
            let n = len.min(BUF_SIZE);
            return f(&mut scratch[..n]);
        }

        let id = self.free_head;
        let next = unsafe { (*self.q.desc.add(id as usize)).next };
        self.free_head = next;
        self.num_free -= 1;

        let va = self.q.buf_va(id);
        let pa = self.q.buf_pa(id);
        let r = unsafe {
            // Header: flags=0, gso_type=0, hdr_len=0, gso_size=0, csum_start=0,
            // csum_offset=0[, num_buffers=1 on the modern transport].
            core::ptr::write_bytes(va as *mut u8, 0, hdr);
            if hdr >= HDR_MODERN {
                core::ptr::write_unaligned((va + 10) as *mut u16, 1u16.to_le());
            }
            let frame = core::slice::from_raw_parts_mut((va + hdr) as *mut u8, len);
            let r = f(frame);
            let d = self.q.desc.add(id as usize);
            (*d).addr = pa as u64;
            (*d).len = (hdr + len) as u32;
            (*d).flags = 0;
            (*d).next = 0;
            r
        };

        self.q.add_avail(id);
        self.packets += 1;
        r
    }
}

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

/// A probed and initialised virtio-net device.
pub struct Net {
    pub base: usize,
    pub slot: usize,
    pub mac: [u8; 6],
    pub features: u64,
    pub transport: Transport,
    /// virtio-net header size used on RX/TX for this device.
    pub hdr_len: usize,
    pub rx: RxQueue,
    pub tx: TxQueue,
    /// Last value read from InterruptStatus.
    pub last_irq_status: u32,
}

impl Net {
    /// Probe one virtio-mmio slot and, if it is a network device, initialise it
    /// completely.  Returns `None` for empty/foreign slots and for any device
    /// that fails validation; never panics.
    pub fn probe(slot: usize) -> Option<Net> {
        let base = SLOT_BASE + slot * SLOT_STRIDE;

        if r32(base, R_MAGIC) != MAGIC_VALUE
            || r32(base, R_VENDOR_ID) != VENDOR_QEMU
            || r32(base, R_DEVICE_ID) != DEVICE_ID_NET
        {
            return None;
        }
        let transport = match r32(base, R_VERSION) {
            1 => Transport::Legacy,
            2 => Transport::Modern,
            _ => return None,
        };
        let hdr_len = match transport {
            Transport::Legacy => HDR_LEGACY,
            Transport::Modern => HDR_MODERN,
        };

        // --- reset, then ACKNOWLEDGE | DRIVER ---
        w32(base, R_STATUS, 0);
        w32(base, R_STATUS, ST_ACKNOWLEDGE);
        w32(base, R_STATUS, ST_ACKNOWLEDGE | ST_DRIVER);
        if transport == Transport::Legacy {
            // Must be set before QueuePFN is written.
            w32(base, R_GUEST_PAGE_SIZE, LEGACY_ALIGN as u32);
        }

        // --- feature negotiation ---
        let features: u64;
        if transport == Transport::Modern {
            w32(base, R_DEVICE_FEATURES_SEL, 0);
            let feat_lo = r32(base, R_DEVICE_FEATURES);
            w32(base, R_DEVICE_FEATURES_SEL, 1);
            let feat_hi = r32(base, R_DEVICE_FEATURES);
            features = ((feat_hi as u64) << 32) | feat_lo as u64;

            // VIRTIO_F_VERSION_1 is mandatory for the modern transport.
            if feat_hi & FEAT_VERSION_1_WORD1 == 0 {
                crate::println!("[net] slot {}: v2 device without VIRTIO_F_VERSION_1", slot);
                w32(base, R_STATUS, 0);
                return None;
            }
            let want_lo = feat_lo & FEAT_MAC;
            let want_hi = feat_hi & FEAT_VERSION_1_WORD1;
            w32(base, R_DRIVER_FEATURES_SEL, 1);
            w32(base, R_DRIVER_FEATURES, want_hi);
            w32(base, R_DRIVER_FEATURES_SEL, 0);
            w32(base, R_DRIVER_FEATURES, want_lo);

            w32(base, R_STATUS, ST_ACKNOWLEDGE | ST_DRIVER | ST_FEATURES_OK);
            if r32(base, R_STATUS) & ST_FEATURES_OK == 0 {
                crate::println!("[net] slot {}: device rejected features {:#x}", slot, want_lo);
                w32(base, R_STATUS, 0);
                return None;
            }
        } else {
            // Legacy: a single 32-bit feature word, no FEATURES_OK.
            let feat = r32(base, R_DEVICE_FEATURES);
            features = feat as u64;
            w32(base, R_DRIVER_FEATURES, feat & FEAT_MAC);
        }

        // --- MAC from device config space (feature-gated) ---
        let mut mac = super::GUEST_MAC;
        if features & FEAT_MAC as u64 != 0 {
            if let Some(m) = read_mac(base, transport) {
                mac = m;
            }
        }

        // --- queues ---
        let rx_q = match Self::setup_queue(base, 0, slot, transport, hdr_len) {
            Some(q) => q,
            None => {
                w32(base, R_STATUS, 0);
                return None;
            }
        };
        let tx_q = match Self::setup_queue(base, 1, slot, transport, hdr_len) {
            Some(q) => q,
            None => {
                w32(base, R_STATUS, 0);
                return None;
            }
        };
        let rx = new_rx_queue(rx_q);
        let tx = new_tx_queue(tx_q);

        // --- driver ok ---
        let mut st = ST_ACKNOWLEDGE | ST_DRIVER | ST_DRIVER_OK;
        if transport == Transport::Modern {
            st |= ST_FEATURES_OK;
        }
        w32(base, R_STATUS, st);

        Some(Net {
            base,
            slot,
            mac,
            features,
            transport,
            hdr_len,
            rx,
            tx,
            last_irq_status: 0,
        })
    }

    fn setup_queue(
        base: usize,
        qidx: u32,
        slot: usize,
        transport: Transport,
        hdr_len: usize,
    ) -> Option<Vq> {
        w32(base, R_QUEUE_SEL, qidx);
        let max = r32(base, R_QUEUE_NUM_MAX) as usize;
        if max == 0 {
            crate::println!("[net] slot {}: queue {} missing", slot, qidx);
            return None;
        }
        // Power of two, at most what the device offers.
        let mut num = (QUEUE_NUM as usize).min(max).next_power_of_two();
        while num > max {
            num >>= 1;
        }
        if num < 2 {
            crate::println!("[net] slot {}: queue {} too small ({})", slot, qidx, max);
            return None;
        }

        let desc_bytes = num * core::mem::size_of::<Desc>();
        let avail_bytes = 6 + 2 * num;
        let used_bytes = 6 + 8 * num;

        // Layout differs between transports:
        //  * modern: any 16/2/4-byte aligned non-overlapping placement;
        //  * legacy: exactly ALIGN(desc+avail, qalign) then ALIGN(used, qalign)
        //    inside the QueuePFN'd pages (virtio 1.1 section 2.6.2).
        let (avail_off, used_off, ring_bytes) = match transport {
            Transport::Modern => {
                let avail_off = desc_bytes;
                let used_off = align_up(avail_off + avail_bytes, 4);
                (avail_off, used_off, used_off + used_bytes)
            }
            Transport::Legacy => {
                let avail_off = desc_bytes;
                let used_off = align_up(avail_off + avail_bytes, LEGACY_ALIGN);
                let total = used_off + align_up(used_bytes, LEGACY_ALIGN);
                (avail_off, used_off, total)
            }
        };

        let ring_frames = (ring_bytes + 4095) / 4096;
        let buf_bytes = num * BUF_SIZE;
        let buf_frames = (buf_bytes + 4095) / 4096;

        let ring_pa = alloc_frames(ring_frames)?;
        let buf_pa = match alloc_frames(buf_frames) {
            Some(p) => p,
            None => {
                free_frames(ring_pa, ring_frames);
                return None;
            }
        };

        // alloc_frames() zeroes the memory, which is exactly the initial state
        // the available/used rings need.
        let q = Vq {
            base,
            qidx,
            num: num as u16,
            hdr_len,
            desc: ring_pa as *mut Desc,
            avail_idx: (ring_pa + avail_off + 2) as *mut u16,
            avail_ring: (ring_pa + avail_off + 4) as *mut u16,
            used_idx: (ring_pa + used_off + 2) as *mut u16,
            used_ring: (ring_pa + used_off + 4) as *mut UsedElem,
            buf_pa,
            buf_va: buf_pa,
            ring_pa,
            ring_frames,
            buf_frames,
        };

        w32(base, R_QUEUE_NUM, num as u32);
        match transport {
            Transport::Modern => {
                w32(base, R_QUEUE_DESC_LOW, ring_pa as u32);
                w32(base, R_QUEUE_DESC_HIGH, (ring_pa >> 32) as u32);
                w32(base, R_QUEUE_DRIVER_LOW, (ring_pa + avail_off) as u32);
                w32(base, R_QUEUE_DRIVER_HIGH, ((ring_pa + avail_off) >> 32) as u32);
                w32(base, R_QUEUE_DEVICE_LOW, (ring_pa + used_off) as u32);
                w32(base, R_QUEUE_DEVICE_HIGH, ((ring_pa + used_off) >> 32) as u32);
                fence(Ordering::SeqCst);
                w32(base, R_QUEUE_READY, 1);
            }
            Transport::Legacy => {
                w32(base, R_QUEUE_ALIGN, LEGACY_ALIGN as u32);
                fence(Ordering::SeqCst);
                w32(base, R_QUEUE_PFN, (ring_pa / LEGACY_ALIGN) as u32);
            }
        }

        Some(q)
    }

    /// Read InterruptStatus and acknowledge it.
    pub fn ack_interrupt(&mut self) {
        let s = r32(self.base, R_INTERRUPT_STATUS);
        self.last_irq_status = s;
        if s != 0 {
            w32(self.base, R_INTERRUPT_ACK, s);
        }
    }

    pub fn device_status(&self) -> u32 {
        r32(self.base, R_STATUS)
    }

    /// Acknowledge interrupts using only the cached MMIO base: safe to call
    /// from an interrupt handler while the stack lock is held elsewhere.
    pub fn ack_interrupt_nolock() {
        let base = IRQ_BASE.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        let s = r32(base, R_INTERRUPT_STATUS);
        if s != 0 {
            w32(base, R_INTERRUPT_ACK, s);
        }
    }

    pub fn publish_irq_base(&self) {
        IRQ_BASE.store(self.base, Ordering::Relaxed);
    }

    /// True when at least one TX descriptor is free.
    pub fn tx_ready(&mut self) -> bool {
        self.tx.reclaim();
        self.tx.num_free > 0
    }
}

/// Read the 6-byte MAC from device config space, retrying until the
/// ConfigGeneration count is stable (virtio 1.1 section 2.4.1; the legacy
/// transport has no generation register, so a single read is used).
fn read_mac(base: usize, transport: Transport) -> Option<[u8; 6]> {
    let read_once = || {
        let mut m = [0u8; 6];
        for (i, b) in m.iter_mut().enumerate() {
            *b = r8(base, R_CONFIG + i);
        }
        m
    };
    match transport {
        Transport::Legacy => Some(read_once()),
        Transport::Modern => {
            for _ in 0..8 {
                let before = r32(base, R_CONFIG_GENERATION);
                let m = read_once();
                let after = r32(base, R_CONFIG_GENERATION);
                if before == after {
                    return Some(m);
                }
            }
            None
        }
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        // Best effort: reset the device and hand the frames back.
        w32(self.base, R_STATUS, 0);
        free_frames(self.rx.q.ring_pa, self.rx.q.ring_frames);
        free_frames(self.rx.q.buf_pa, self.rx.q.buf_frames);
        free_frames(self.tx.q.ring_pa, self.tx.q.ring_frames);
        free_frames(self.tx.q.buf_pa, self.tx.q.buf_frames);
    }
}

// ---------------------------------------------------------------------------
// Queue construction (initial descriptor state)
// ---------------------------------------------------------------------------

fn new_rx_queue(q: Vq) -> RxQueue {
    // Post every buffer as device-writable and publish them all at once.
    let n = q.num();
    for i in 0..n {
        let id = i as u16;
        unsafe {
            let d = q.desc.add(i);
            (*d).addr = q.buf_pa(id) as u64;
            (*d).len = BUF_SIZE as u32;
            (*d).flags = VIRTQ_DESC_F_WRITE;
            (*d).next = 0;
            core::ptr::write_volatile(q.avail_ring.add(i), id);
        }
    }
    unsafe {
        core::ptr::write_volatile(q.avail_idx, n as u16);
    }
    q.notify();

    RxQueue {
        q,
        last_used: 0,
        packets: 0,
        malformed: 0,
    }
}

fn new_tx_queue(q: Vq) -> TxQueue {
    let n = q.num();
    for i in 0..n {
        unsafe {
            let d = q.desc.add(i);
            (*d).addr = q.buf_pa(i as u16) as u64;
            (*d).len = 0;
            (*d).flags = 0;
            (*d).next = if i + 1 < n { (i + 1) as u16 } else { 0 };
        }
    }
    TxQueue {
        q,
        last_used: 0,
        free_head: 0,
        num_free: n as u16,
        packets: 0,
        dropped: 0,
    }
}
