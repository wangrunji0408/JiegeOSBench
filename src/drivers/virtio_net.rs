//! virtio-net driver.

use super::virtio::{MmioTransport, VirtQueue, VIRTIO_F_VERSION_1};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

/// virtio-net feature bits.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;

/// Queue indices.
const RX_QUEUE: u32 = 0;
const TX_QUEUE: u32 = 1;

/// Queue depth. Larger queues let us keep more RX buffers posted, which matters
/// because we only refill them when polling.
const QUEUE_SIZE: u16 = 256;
/// Maximum frame we handle: an Ethernet MTU plus headers.
const BUFFER_SIZE: usize = 2048;

/// The virtio-net header that precedes every frame.
///
/// For a non-legacy (virtio 1.0) device the header is always 12 bytes: the
/// `num_buffers` field is present regardless of whether `VIRTIO_NET_F_MRG_RXBUF`
/// was negotiated. Getting the size wrong shifts every frame, which corrupts the
/// Ethernet header.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,
}

/// Size of the header the device prepends.
const HDR_SIZE: usize = core::mem::size_of::<VirtioNetHdr>();
const _: () = assert!(HDR_SIZE == 12, "virtio 1.0 net header is 12 bytes");

/// A buffer used for a queued RX or TX request: the header followed by the frame.
///
/// Buffers are allocated once at initialization and reused for the life of the
/// device. Allocating per frame would work, but it puts the device's DMA target
/// at the mercy of the kernel allocator's reuse decisions on a hot path — a
/// buffer freed while a descriptor still referenced it would be silently
/// overwritten. Owning them outright makes that impossible.
struct Buffer {
    data: Box<[u8; BUFFER_SIZE]>,
}

impl Buffer {
    fn new() -> Self {
        // Allocate zeroed; the header must start clean for TX.
        Self {
            data: Box::new([0u8; BUFFER_SIZE]),
        }
    }

    fn addr(&self) -> usize {
        self.data.as_ptr() as usize
    }

    /// The frame portion, after the virtio header.
    fn frame(&self) -> &[u8] {
        &self.data[HDR_SIZE..]
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.data[HDR_SIZE..]
    }
}

pub struct VirtioNet {
    transport: MmioTransport,
    rx_queue: VirtQueue,
    tx_queue: VirtQueue,
    /// Buffers posted to the RX queue, indexed by descriptor head.
    rx_buffers: Vec<Option<Buffer>>,
    /// Buffers in flight on the TX queue, indexed by descriptor head.
    tx_buffers: Vec<Option<Buffer>>,
    /// Frames received and waiting to be handed to the network stack.
    rx_ready: VecDeque<Vec<u8>>,
    /// RX buffers not currently posted to the device, ready to be reposted.
    rx_free: Vec<Buffer>,
    /// TX buffers we can reuse.
    tx_free: Vec<Buffer>,
    pub mac: [u8; 6],
    pub rx_packets: usize,
    pub tx_packets: usize,
    pub rx_dropped: usize,
    pub tx_dropped: usize,
}

static DEVICE: Mutex<Option<VirtioNet>> = Mutex::new(None);

/// Run `f` with the device locked and interrupts disabled.
///
/// The PLIC handler takes this same lock, and `spin::Mutex` is not reentrant, so
/// every acquisition must mask interrupts or an IRQ arriving mid-critical-section
/// deadlocks the hart.
fn with_device<T>(f: impl FnOnce(&mut VirtioNet) -> T) -> Option<T> {
    crate::trap::without_interrupts(|| DEVICE.lock().as_mut().map(f))
}

/// Initialize a virtio-net device.
pub fn init(transport: MmioTransport) -> Result<(), &'static str> {
    let wanted = VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS;
    let accepted = transport.begin_init(wanted)?;

    let mac = if accepted & VIRTIO_NET_F_MAC != 0 {
        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = transport.read_config_u8(i);
        }
        mac
    } else {
        // Make one up, with the locally-administered bit set.
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
    };

    let rx_queue = VirtQueue::new(QUEUE_SIZE).ok_or("cannot allocate RX queue")?;
    let tx_queue = VirtQueue::new(QUEUE_SIZE).ok_or("cannot allocate TX queue")?;
    transport.setup_queue(RX_QUEUE, &rx_queue)?;
    transport.setup_queue(TX_QUEUE, &tx_queue)?;

    let irq = transport.irq;
    let mut device = VirtioNet {
        transport,
        rx_queue,
        tx_queue,
        rx_buffers: (0..QUEUE_SIZE).map(|_| None).collect(),
        tx_buffers: (0..QUEUE_SIZE).map(|_| None).collect(),
        rx_ready: VecDeque::new(),
        // Allocate the whole RX buffer pool up front, once.
        rx_free: (0..QUEUE_SIZE).map(|_| Buffer::new()).collect(),
        tx_free: Vec::new(),
        mac,
        rx_packets: 0,
        tx_packets: 0,
        rx_dropped: 0,
        tx_dropped: 0,
    };

    // Post RX buffers before telling the device we are ready.
    device.refill_rx();
    device.transport.finish_init();
    crate::info!(
        "virtio-net: features accepted {:#x}, rx free {} of {}",
        accepted,
        device.rx_queue.free_count(),
        QUEUE_SIZE,
    );

    crate::info!(
        "virtio-net: mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, {} rx buffers posted",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        QUEUE_SIZE as usize - device.rx_queue.free_count() as usize,
    );

    *DEVICE.lock() = Some(device);
    super::plic::register(irq, on_interrupt);
    Ok(())
}

impl VirtioNet {
    /// Hand every free RX buffer back to the device.
    fn refill_rx(&mut self) {
        let mut posted = 0;
        while let Some(buffer) = self.rx_free.pop() {
            if self.rx_queue.free_count() < 1 {
                self.rx_free.push(buffer);
                break;
            }
            let addr = buffer.addr();
            // The whole buffer is device-writable: header plus frame.
            match self.rx_queue.add(&[], &[(addr, BUFFER_SIZE)]) {
                Some(head) => {
                    debug_assert!(
                        self.rx_buffers[head as usize].is_none(),
                        "reposting a descriptor whose buffer the device still owns",
                    );
                    self.rx_buffers[head as usize] = Some(buffer);
                    posted += 1;
                }
                None => {
                    self.rx_free.push(buffer);
                    break;
                }
            }
        }
        // Only kick the device when we actually gave it something new.
        if posted > 0 {
            self.transport.notify(RX_QUEUE);
        }
    }

    /// Collect completed RX descriptors into `rx_ready`.
    fn collect_rx(&mut self) {
        // Drain every completion before reposting. `refill_rx` hands descriptors
        // back to the device, and a descriptor index it reuses would clobber a
        // `rx_buffers` slot we have not yet taken — so the two phases must not
        // interleave.
        while let Some((head, len)) = self.rx_queue.pop() {
            let Some(buffer) = self.rx_buffers[head as usize].take() else {
                crate::warn!("virtio-net: RX completion for unknown descriptor {}", head);
                continue;
            };
            let len = len as usize;
            if len > HDR_SIZE && len <= BUFFER_SIZE {
                let frame_len = len - HDR_SIZE;
                // Bound the backlog: if the stack isn't draining, dropping is
                // better than growing the heap without limit.
                if self.rx_ready.len() < 1024 {
                    self.rx_ready
                        .push_back(buffer.frame()[..frame_len].to_vec());
                    self.rx_packets += 1;
                } else {
                    self.rx_dropped += 1;
                }
            }
            // Recycle the buffer rather than freeing it.
            self.rx_free.push(buffer);
        }
        self.refill_rx();
    }

    /// Reclaim completed TX descriptors.
    fn collect_tx(&mut self) {
        while let Some((head, _)) = self.tx_queue.pop() {
            if let Some(buffer) = self.tx_buffers[head as usize].take() {
                if self.tx_free.len() < 64 {
                    self.tx_free.push(buffer);
                }
            }
        }
    }

    /// Take the next received frame.
    fn receive(&mut self) -> Option<Vec<u8>> {
        if self.rx_ready.is_empty() {
            self.collect_rx();
        }
        self.rx_ready.pop_front()
    }

    /// Queue a frame for transmission.
    fn transmit(&mut self, frame: &[u8]) -> bool {
        if frame.len() > BUFFER_SIZE - HDR_SIZE {
            crate::warn!("virtio-net: frame of {} bytes is too large", frame.len());
            return false;
        }
        self.collect_tx();
        if self.tx_queue.free_count() < 1 {
            // The queue is full and nothing has completed yet. Dropping here
            // would be silently lossy: smoltcp treats a consumed TxToken as sent
            // and advances its send window, so the segment is gone rather than
            // retransmitted promptly, and the connection stalls until the peer's
            // retransmit timer fires seconds later.
            //
            // Spin briefly for a completion instead. The device drains the queue
            // in microseconds, so this is a much shorter wait than the stall it
            // avoids, and it cannot deadlock: we hold no lock the device needs.
            for _ in 0..10_000 {
                core::hint::spin_loop();
                self.collect_tx();
                if self.tx_queue.free_count() >= 1 {
                    break;
                }
            }
            if self.tx_queue.free_count() < 1 {
                self.tx_dropped += 1;
                return false;
            }
        }
        let mut buffer = self.tx_free.pop().unwrap_or_else(Buffer::new);
        // Clear the header and copy the frame in after it.
        buffer.data[..HDR_SIZE].fill(0);
        buffer.frame_mut()[..frame.len()].copy_from_slice(frame);
        let addr = buffer.addr();
        let total = HDR_SIZE + frame.len();

        match self.tx_queue.add(&[(addr, total)], &[]) {
            Some(head) => {
                self.tx_buffers[head as usize] = Some(buffer);
                self.transport.notify(TX_QUEUE);
                self.tx_packets += 1;
                true
            }
            None => {
                self.tx_free.push(buffer);
                false
            }
        }
    }
}

/// Interrupts seen, for diagnosing a silent device.
pub static IRQ_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The PLIC handler. Runs with interrupts already disabled by the trap entry.
fn on_interrupt() {
    IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    // Only drain the queues here. Calling into smoltcp from the interrupt
    // handler would take the network stack's lock, which a task may already hold
    // — the frames sit in `rx_ready` until the next poll instead.
    let mut guard = DEVICE.lock();
    if let Some(device) = guard.as_mut() {
        device.transport.ack_interrupt();
        device.collect_rx();
        device.collect_tx();
    }
}

/// Is a network device present?
pub fn present() -> bool {
    crate::trap::without_interrupts(|| DEVICE.lock().is_some())
}

pub fn mac_address() -> Option<[u8; 6]> {
    crate::trap::without_interrupts(|| DEVICE.lock().as_ref().map(|d| d.mac))
}

/// Take the next received frame, if any.
pub fn receive() -> Option<Vec<u8>> {
    with_device(|d| d.receive()).flatten()
}

/// Send a frame. Returns false if it was dropped.
pub fn transmit(frame: &[u8]) -> bool {
    with_device(|d| d.transmit(frame)).unwrap_or(false)
}

/// Poll the device for completions without waiting for an interrupt.
pub fn poll_device() {
    with_device(|d| {
        d.collect_rx();
        d.collect_tx();
    });
}

/// Same as [`poll_device`], for callers that already hold their own lock with
/// interrupts masked (the network stack's `poll`).
pub fn poll_device_locked() {
    if let Some(device) = DEVICE.lock().as_mut() {
        device.collect_rx();
        device.collect_tx();
    }
}

/// Are there received frames the network stack has not consumed yet?
///
/// Called with interrupts already masked by the network stack's poll loop.
pub fn has_pending_rx() -> bool {
    match DEVICE.lock().as_mut() {
        Some(device) => {
            if device.rx_ready.is_empty() {
                // A completion may have landed since the last drain.
                device.collect_rx();
            }
            !device.rx_ready.is_empty()
        }
        None => false,
    }
}

/// Diagnostic snapshot of the RX path: (posted, pending_completions, ready).
pub fn rx_debug() -> (u16, u16, usize) {
    with_device(|d| {
        (
            QUEUE_SIZE - d.rx_queue.free_count(),
            d.rx_queue.pending(),
            d.rx_ready.len(),
        )
    })
    .unwrap_or((0, 0, 0))
}

/// (rx_packets, tx_packets, rx_dropped, tx_dropped)
pub fn stats() -> (usize, usize, usize, usize) {
    with_device(|d| (d.rx_packets, d.tx_packets, d.rx_dropped, d.tx_dropped))
        .unwrap_or((0, 0, 0, 0))
}
