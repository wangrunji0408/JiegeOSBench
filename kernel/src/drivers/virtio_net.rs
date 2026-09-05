//! virtio-net over virtio-mmio (QEMU virt) using the `virtio-drivers` raw API,
//! exposed as a smoltcp `Device`. Transmission is asynchronous: packets are
//! queued to the device and completed buffers are reclaimed lazily.
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use virtio_drivers::device::net::VirtIONetRaw;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

use crate::config::PAGE_SIZE;
use crate::sync::Global;

pub struct KernelHal;

unsafe impl Hal for KernelHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let layout = core::alloc::Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
        assert!(!p.is_null(), "dma_alloc failed");
        (p as usize as PhysAddr, NonNull::new(p).unwrap())
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let layout = core::alloc::Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE).unwrap();
        unsafe { alloc::alloc::dealloc(vaddr.as_ptr(), layout) };
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        buffer.as_ptr() as *mut u8 as usize as PhysAddr
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

const QUEUE_SIZE: usize = 64;
const BUF_LEN: usize = 2048;

pub type Net = VirtIONetRaw<KernelHal, MmioTransport<'static>, QUEUE_SIZE>;

type Buf = Box<[u8; BUF_LEN]>;

fn new_buf() -> Buf {
    Box::new([0u8; BUF_LEN])
}

pub struct NetDevice {
    net: Net,
    /// Receive buffers currently owned by the device, indexed by token.
    rx_bufs: Vec<Option<Buf>>,
    /// Transmit buffers currently owned by the device, indexed by token.
    tx_bufs: Vec<Option<Buf>>,
    /// Spare buffers for transmission.
    tx_pool: Vec<Buf>,
    hdr_len: usize,
}

static IRQ: AtomicUsize = AtomicUsize::new(0);
pub static DEVICE: Global<NetDevice> = Global::new();

pub fn irq() -> usize {
    IRQ.load(Ordering::Relaxed)
}

impl NetDevice {
    fn new(mut net: Net) -> Self {
        let mut probe = [0u8; 64];
        let hdr_len = net.fill_buffer_header(&mut probe).unwrap();
        let mut rx_bufs: Vec<Option<Buf>> = (0..QUEUE_SIZE).map(|_| None).collect();
        // Fill the receive queue (leave a couple of descriptors spare).
        for _ in 0..QUEUE_SIZE - 2 {
            let mut b = new_buf();
            match unsafe { net.receive_begin(&mut b[..]) } {
                Ok(token) => rx_bufs[token as usize] = Some(b),
                Err(_) => break,
            }
        }
        let tx_pool = (0..QUEUE_SIZE).map(|_| new_buf()).collect();
        NetDevice { net, rx_bufs, tx_bufs: (0..QUEUE_SIZE).map(|_| None).collect(), tx_pool, hdr_len }
    }

    /// Return completed transmit buffers to the pool.
    fn reclaim_tx(&mut self) {
        while let Some(token) = self.net.poll_transmit() {
            let Some(buf) = self.tx_bufs[token as usize].take() else {
                klog!("virtio-net: tx completion for unknown token {}", token);
                break;
            };
            let _ = unsafe { self.net.transmit_complete(token, &buf[..]) };
            self.tx_pool.push(buf);
        }
    }

    fn can_transmit(&mut self) -> bool {
        if self.net.can_send() && !self.tx_pool.is_empty() {
            return true;
        }
        self.reclaim_tx();
        self.net.can_send() && !self.tx_pool.is_empty()
    }

    fn transmit_packet(&mut self, len: usize, f: impl FnOnce(&mut [u8])) {
        let Some(mut buf) = self.tx_pool.pop() else { return };
        let total = self.hdr_len + len;
        if total > BUF_LEN {
            klog!("virtio-net: dropping oversized packet ({} bytes)", len);
            self.tx_pool.push(buf);
            return;
        }
        self.net.fill_buffer_header(&mut buf[..]).unwrap();
        f(&mut buf[self.hdr_len..total]);
        match unsafe { self.net.transmit_begin(&buf[..total]) } {
            Ok(token) => self.tx_bufs[token as usize] = Some(buf),
            Err(e) => {
                klog!("virtio-net: transmit_begin failed: {:?}", e);
                self.tx_pool.push(buf);
            }
        }
    }

    /// Take a received packet, if any: (buffer, packet range).
    fn receive_packet(&mut self) -> Option<(Buf, usize, usize)> {
        let token = self.net.poll_receive()?;
        let mut buf = self.rx_bufs[token as usize].take()?;
        match unsafe { self.net.receive_complete(token, &mut buf[..]) } {
            Ok((hdr, len)) => Some((buf, hdr, len)),
            Err(e) => {
                klog!("virtio-net: receive_complete failed: {:?}", e);
                self.recycle_rx(buf);
                None
            }
        }
    }

    fn recycle_rx(&mut self, mut buf: Buf) {
        match unsafe { self.net.receive_begin(&mut buf[..]) } {
            Ok(token) => self.rx_bufs[token as usize] = Some(buf),
            Err(e) => klog!("virtio-net: receive_begin failed: {:?}", e),
        }
    }

    fn recycle_pending(&mut self) {
        let bufs: Vec<Buf> = core::mem::take(&mut *RECYCLE.lock());
        for b in bufs {
            self.recycle_rx(b);
        }
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.net.mac_address()
    }
}

static RECYCLE: crate::sync::SpinLock<Vec<Buf>> = crate::sync::SpinLock::new(Vec::new());

/// Probe the 8 virtio-mmio slots of the virt machine for a network device.
pub fn init() -> bool {
    for i in 0..8 {
        let base = 0x1000_1000 + i * 0x1000;
        let header = NonNull::new(base as *mut VirtIOHeader).unwrap();
        let transport = match unsafe { MmioTransport::new(header, 0x1000) } {
            Ok(t) => t,
            Err(_) => continue,
        };
        if transport.device_type() != DeviceType::Network {
            continue;
        }
        match VirtIONetRaw::<KernelHal, _, QUEUE_SIZE>::new(transport) {
            Ok(net) => {
                let irq = i + 1;
                IRQ.store(irq, Ordering::Relaxed);
                klog!("virtio-net at {:#x}, irq {}, mac {:02x?}", base, irq, net.mac_address());
                DEVICE.init(NetDevice::new(net));
                super::plic::enable(irq);
                return true;
            }
            Err(e) => {
                klog!("virtio-net init failed: {:?}", e);
                return false;
            }
        }
    }
    false
}

pub fn handle_irq() {
    if DEVICE.is_init() {
        let _ = DEVICE.get().net.ack_interrupt();
        crate::net::poll();
    }
}

pub fn mac() -> [u8; 6] {
    DEVICE.get().mac_address()
}

// ---- smoltcp Device impl ----

pub struct VirtioRxToken {
    buf: Option<Buf>,
    hdr: usize,
    len: usize,
}

impl Drop for VirtioRxToken {
    fn drop(&mut self) {
        if let Some(b) = self.buf.take() {
            RECYCLE.lock().push(b);
        }
    }
}

pub struct VirtioTxToken<'a>(&'a mut NetDevice);

impl Device for NetDevice {
    type RxToken<'a> = VirtioRxToken
    where
        Self: 'a;
    type TxToken<'a> = VirtioTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.recycle_pending();
        let (buf, hdr, len) = self.receive_packet()?;
        Some((VirtioRxToken { buf: Some(buf), hdr, len }, VirtioTxToken(self)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.can_transmit() {
            Some(VirtioTxToken(self))
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(QUEUE_SIZE / 2);
        caps.medium = Medium::Ethernet;
        caps
    }
}

impl phy::RxToken for VirtioRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let mut this = self;
        let buf = this.buf.take().unwrap();
        let r = f(&buf[this.hdr..this.hdr + this.len]);
        RECYCLE.lock().push(buf);
        r
    }
}

impl phy::TxToken for VirtioTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut result = None;
        self.0.transmit_packet(len, |pkt| {
            result = Some(f(pkt));
        });
        match result {
            Some(r) => r,
            None => {
                // No buffer available: let smoltcp build into a scratch buffer.
                let mut scratch = alloc::vec![0u8; len];
                f(&mut scratch)
            }
        }
    }
}
