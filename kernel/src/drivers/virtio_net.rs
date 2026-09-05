//! virtio-net over virtio-mmio (QEMU virt) using the `virtio-drivers` crate,
//! exposed as a smoltcp `Device`.
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use virtio_drivers::device::net::{RxBuffer, VirtIONet};
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
        (p as usize, NonNull::new(p).unwrap())
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
        buffer.as_ptr() as *mut u8 as usize
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}

const QUEUE_SIZE: usize = 32;
const BUF_LEN: usize = 2048;

pub type Net = VirtIONet<KernelHal, MmioTransport<'static>, QUEUE_SIZE>;

pub struct NetDevice {
    pub net: Net,
}

static IRQ: AtomicUsize = AtomicUsize::new(0);
pub static DEVICE: Global<NetDevice> = Global::new();

pub fn irq() -> usize {
    IRQ.load(Ordering::Relaxed)
}

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
        match VirtIONet::<KernelHal, _, QUEUE_SIZE>::new(transport, BUF_LEN) {
            Ok(net) => {
                let irq = i + 1;
                IRQ.store(irq, Ordering::Relaxed);
                klog!("virtio-net at {:#x}, irq {}, mac {:02x?}", base, irq, net.mac_address());
                DEVICE.init(NetDevice { net });
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
    DEVICE.get().net.mac_address()
}

// ---- smoltcp Device impl ----

pub struct VirtioRxToken(RxBuffer);
pub struct VirtioTxToken<'a>(&'a mut Net);

impl Device for NetDevice {
    type RxToken<'a> = VirtioRxToken
    where
        Self: 'a;
    type TxToken<'a> = VirtioTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.net.receive() {
            Ok(buf) => Some((VirtioRxToken(buf), VirtioTxToken(&mut self.net))),
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if self.net.can_send() {
            Some(VirtioTxToken(&mut self.net))
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
        let mut buf = self.0;
        let r = f(buf.packet_mut());
        // recycle
        let dev = DEVICE.get();
        if let Err(e) = dev.net.recycle_rx_buffer(buf) {
            klog!("recycle_rx_buffer failed: {:?}", e);
        }
        r
    }
}

impl phy::TxToken for VirtioTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut tx = self.0.new_tx_buffer(len);
        let r = f(tx.packet_mut());
        if let Err(e) = self.0.send(tx) {
            klog!("virtio-net send failed: {:?}", e);
        }
        r
    }
}
