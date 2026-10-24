use alloc::{vec, vec::Vec};
use core::mem::size_of;
use core::ptr::{read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{Ordering, compiler_fence};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::memory::alloc_frame;

const QUEUE_SIZE: usize = 16;
const PAGE_SIZE: usize = 4096;
const NET_HEADER_LEN: usize = 12;
const MAX_PACKET_SIZE: usize = PAGE_SIZE - NET_HEADER_LEN;
const ETHERNET_MTU: usize = 1514;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

const MMIO_MAGIC_VALUE: usize = 0x000;
const MMIO_VERSION: usize = 0x004;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_DEVICE_FEATURES: usize = 0x010;
const MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
const MMIO_DRIVER_FEATURES: usize = 0x020;
const MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
const MMIO_QUEUE_SEL: usize = 0x030;
const MMIO_QUEUE_NUM_MAX: usize = 0x034;
const MMIO_QUEUE_NUM: usize = 0x038;
const MMIO_QUEUE_READY: usize = 0x044;
const MMIO_QUEUE_NOTIFY: usize = 0x050;
const MMIO_INTERRUPT_STATUS: usize = 0x060;
const MMIO_INTERRUPT_ACK: usize = 0x064;
const MMIO_STATUS: usize = 0x070;
const MMIO_QUEUE_DESC_LOW: usize = 0x080;
const MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const MMIO_QUEUE_AVAIL_LOW: usize = 0x090;
const MMIO_QUEUE_AVAIL_HIGH: usize = 0x094;
const MMIO_QUEUE_USED_LOW: usize = 0x0a0;
const MMIO_QUEUE_USED_HIGH: usize = 0x0a4;

const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_VERSION_MODERN: u32 = 2;
const VIRTIO_DEVICE_NET: u32 = 1;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_FAILED: u32 = 128;

const VIRTIO_F_VERSION_1_HIGH: u32 = 1;
const DESC_F_WRITE: u16 = 2;

#[derive(Clone, Copy)]
struct Mmio {
    base: usize,
}

impl Mmio {
    #[inline]
    fn read(self, offset: usize) -> u32 {
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    #[inline]
    fn write(self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, value) };
    }

    fn add_status(self, bits: u32) {
        let status = self.read(MMIO_STATUS);
        self.write(MMIO_STATUS, status | bits);
    }

    fn notify(self, queue: u16) {
        compiler_fence(Ordering::SeqCst);
        self.write(MMIO_QUEUE_NOTIFY, queue as u32);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct AvailRing {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C)]
struct UsedRing {
    flags: u16,
    index: u16,
    ring: [UsedElement; QUEUE_SIZE],
}

const _: [(); 16] = [(); size_of::<Descriptor>()];
const _: [(); 4 + QUEUE_SIZE * 2] = [(); size_of::<AvailRing>()];
const _: [(); 4 + QUEUE_SIZE * 8] = [(); size_of::<UsedRing>()];

struct Virtqueue {
    desc: usize,
    avail: usize,
    used: usize,
    avail_index: u16,
}

impl Virtqueue {
    fn allocate(mmio: Mmio, queue: u16) -> Result<Self, &'static str> {
        mmio.write(MMIO_QUEUE_SEL, queue as u32);
        if mmio.read(MMIO_QUEUE_READY) != 0 {
            return Err("virtio-net queue is already active");
        }
        if (mmio.read(MMIO_QUEUE_NUM_MAX) as usize) < QUEUE_SIZE {
            return Err("virtio-net queue is missing or smaller than 16 entries");
        }

        let desc = allocate_page()?;
        let avail = allocate_page()?;
        let used = allocate_page()?;

        mmio.write(MMIO_QUEUE_NUM, QUEUE_SIZE as u32);
        mmio.write(MMIO_QUEUE_DESC_LOW, desc as u32);
        mmio.write(MMIO_QUEUE_DESC_HIGH, (desc >> 32) as u32);
        mmio.write(MMIO_QUEUE_AVAIL_LOW, avail as u32);
        mmio.write(MMIO_QUEUE_AVAIL_HIGH, (avail >> 32) as u32);
        mmio.write(MMIO_QUEUE_USED_LOW, used as u32);
        mmio.write(MMIO_QUEUE_USED_HIGH, (used >> 32) as u32);
        compiler_fence(Ordering::SeqCst);
        mmio.write(MMIO_QUEUE_READY, 1);
        if mmio.read(MMIO_QUEUE_READY) != 1 {
            return Err("virtio-net queue could not be made ready");
        }

        Ok(Self {
            desc,
            avail,
            used,
            avail_index: 0,
        })
    }

    #[inline]
    fn avail_ptr(&self) -> *mut AvailRing {
        self.avail as *mut AvailRing
    }

    #[inline]
    fn used_ptr(&self) -> *mut UsedRing {
        self.used as *mut UsedRing
    }

    fn set_descriptor(&self, index: u16, address: usize, length: u32, flags: u16) {
        let descriptor = Descriptor {
            address: address as u64,
            length,
            flags,
            next: 0,
        };
        unsafe {
            write_volatile(
                (self.desc as *mut Descriptor).add(index as usize),
                descriptor,
            );
        }
    }

    fn push_available(&mut self, descriptor: u16) {
        let slot = (self.avail_index as usize) % QUEUE_SIZE;
        unsafe {
            let ring = core::ptr::addr_of_mut!((*self.avail_ptr()).ring).cast::<u16>();
            write_volatile(ring.add(slot), descriptor);
        }
        compiler_fence(Ordering::SeqCst);
        self.avail_index = self.avail_index.wrapping_add(1);
        unsafe {
            write_volatile(
                core::ptr::addr_of_mut!((*self.avail_ptr()).index),
                self.avail_index,
            );
        }
        compiler_fence(Ordering::SeqCst);
    }

    fn used_index(&self) -> u16 {
        unsafe { read_volatile(core::ptr::addr_of!((*self.used_ptr()).index)) }
    }

    fn used_element(&self, index: u16) -> UsedElement {
        let slot = (index as usize) % QUEUE_SIZE;
        unsafe {
            let ring = core::ptr::addr_of!((*self.used_ptr()).ring).cast::<UsedElement>();
            read_volatile(ring.add(slot))
        }
    }
}

pub struct VirtioNet {
    mmio: Mmio,
    mac: [u8; 6],
    rx: Virtqueue,
    tx: Virtqueue,
    rx_buffers: [usize; QUEUE_SIZE],
    tx_buffers: [usize; QUEUE_SIZE],
    rx_used_index: u16,
    tx_used_index: u16,
    tx_in_use: [bool; QUEUE_SIZE],
    tx_free: usize,
}

impl VirtioNet {
    pub fn new(base: usize, mac: [u8; 6]) -> Result<Self, &'static str> {
        let mmio = Mmio { base };
        if mmio.read(MMIO_MAGIC_VALUE) != VIRTIO_MAGIC {
            return Err("virtio-mmio magic value mismatch");
        }
        if mmio.read(MMIO_VERSION) != VIRTIO_VERSION_MODERN {
            return Err("virtio-mmio device is not modern version 2");
        }
        if mmio.read(MMIO_DEVICE_ID) != VIRTIO_DEVICE_NET {
            return Err("virtio-mmio device is not a network device");
        }

        let result = Self::initialize(mmio, mac);
        if result.is_err() {
            mmio.add_status(STATUS_FAILED);
        }
        result
    }

    fn initialize(mmio: Mmio, mac: [u8; 6]) -> Result<Self, &'static str> {
        mmio.write(MMIO_STATUS, 0);
        let mut reset_complete = false;
        for _ in 0..1_000_000 {
            if mmio.read(MMIO_STATUS) == 0 {
                reset_complete = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !reset_complete {
            return Err("virtio-net device reset timed out");
        }

        mmio.add_status(STATUS_ACKNOWLEDGE);
        Self::checked_status(mmio)?;
        mmio.add_status(STATUS_DRIVER);
        Self::checked_status(mmio)?;

        mmio.write(MMIO_DEVICE_FEATURES_SEL, 1);
        if mmio.read(MMIO_DEVICE_FEATURES) & VIRTIO_F_VERSION_1_HIGH == 0 {
            return Err("virtio-net device does not offer VIRTIO_F_VERSION_1");
        }
        mmio.write(MMIO_DRIVER_FEATURES_SEL, 0);
        mmio.write(MMIO_DRIVER_FEATURES, 0);
        mmio.write(MMIO_DRIVER_FEATURES_SEL, 1);
        mmio.write(MMIO_DRIVER_FEATURES, VIRTIO_F_VERSION_1_HIGH);
        mmio.add_status(STATUS_FEATURES_OK);
        let status = Self::checked_status(mmio)?;
        if status & STATUS_FEATURES_OK == 0 {
            return Err("virtio-net device rejected feature negotiation");
        }

        let mut rx = Virtqueue::allocate(mmio, RX_QUEUE)?;
        let tx = Virtqueue::allocate(mmio, TX_QUEUE)?;
        Self::checked_status(mmio)?;

        let mut rx_buffers = [0; QUEUE_SIZE];
        for index in 0..QUEUE_SIZE {
            let buffer = allocate_page()?;
            rx_buffers[index] = buffer;
            rx.set_descriptor(index as u16, buffer, PAGE_SIZE as u32, DESC_F_WRITE);
            rx.push_available(index as u16);
        }

        let mut tx_buffers = [0; QUEUE_SIZE];
        for buffer in &mut tx_buffers {
            *buffer = allocate_page()?;
        }

        mmio.add_status(STATUS_DRIVER_OK);
        let status = Self::checked_status(mmio)?;
        if status & STATUS_DRIVER_OK == 0 {
            return Err("virtio-net device did not enter DRIVER_OK");
        }

        let device = Self {
            mmio,
            mac,
            rx,
            tx,
            rx_buffers,
            tx_buffers,
            rx_used_index: 0,
            tx_used_index: 0,
            tx_in_use: [false; QUEUE_SIZE],
            tx_free: QUEUE_SIZE,
        };
        device.mmio.notify(RX_QUEUE);
        Ok(device)
    }

    fn checked_status(mmio: Mmio) -> Result<u32, &'static str> {
        let status = mmio.read(MMIO_STATUS);
        if status & STATUS_FAILED != 0 {
            Err("virtio-net device entered FAILED state")
        } else {
            Ok(status)
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn interrupt_ack(&self) -> u32 {
        let status = self.mmio.read(MMIO_INTERRUPT_STATUS);
        if status != 0 {
            self.mmio.write(MMIO_INTERRUPT_ACK, status);
        }
        status
    }

    pub fn poll_rx(&mut self) -> Option<Vec<u8>> {
        compiler_fence(Ordering::SeqCst);
        let used_index = self.rx.used_index();
        compiler_fence(Ordering::SeqCst);
        if self.rx_used_index == used_index {
            return None;
        }

        let element = self.rx.used_element(self.rx_used_index);
        self.rx_used_index = self.rx_used_index.wrapping_add(1);
        compiler_fence(Ordering::SeqCst);

        let descriptor = element.id as usize;
        if descriptor >= QUEUE_SIZE {
            return None;
        }
        let buffer = self.rx_buffers[descriptor];
        let length = element.length as usize;
        let packet = if (NET_HEADER_LEN..=PAGE_SIZE).contains(&length) {
            let packet_length = length - NET_HEADER_LEN;
            let mut packet = vec![0; packet_length];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (buffer + NET_HEADER_LEN) as *const u8,
                    packet.as_mut_ptr(),
                    packet_length,
                );
            }
            Some(packet)
        } else {
            None
        };

        compiler_fence(Ordering::SeqCst);
        self.rx
            .set_descriptor(descriptor as u16, buffer, PAGE_SIZE as u32, DESC_F_WRITE);
        self.rx.push_available(descriptor as u16);
        self.mmio.notify(RX_QUEUE);
        packet
    }

    pub fn poll_tx(&mut self) {
        compiler_fence(Ordering::SeqCst);
        let used_index = self.tx.used_index();
        compiler_fence(Ordering::SeqCst);
        while self.tx_used_index != used_index {
            let element = self.tx.used_element(self.tx_used_index);
            self.tx_used_index = self.tx_used_index.wrapping_add(1);
            compiler_fence(Ordering::SeqCst);
            let descriptor = element.id as usize;
            if descriptor < QUEUE_SIZE && self.tx_in_use[descriptor] {
                self.tx_in_use[descriptor] = false;
                self.tx_free += 1;
            }
        }
    }

    fn has_rx_packet(&self) -> bool {
        compiler_fence(Ordering::SeqCst);
        let used_index = self.rx.used_index();
        compiler_fence(Ordering::SeqCst);
        self.rx_used_index != used_index
    }

    fn reserve_tx(&mut self) -> Option<u16> {
        self.poll_tx();
        if self.tx_free == 0 {
            return None;
        }
        let descriptor = self.tx_in_use.iter().position(|in_use| !in_use)?;
        self.tx_in_use[descriptor] = true;
        self.tx_free -= 1;
        Some(descriptor as u16)
    }

    fn release_tx(&mut self, descriptor: u16) {
        let descriptor = descriptor as usize;
        debug_assert!(descriptor < QUEUE_SIZE && self.tx_in_use[descriptor]);
        self.tx_in_use[descriptor] = false;
        self.tx_free += 1;
    }

    fn transmit_reserved<R, F>(&mut self, descriptor: u16, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        assert!(
            length <= MAX_PACKET_SIZE,
            "virtio-net transmit packet exceeds its buffer"
        );
        let buffer = self.tx_buffers[descriptor as usize];
        unsafe {
            write_bytes(buffer as *mut u8, 0, NET_HEADER_LEN);
        }
        let packet = unsafe {
            core::slice::from_raw_parts_mut((buffer + NET_HEADER_LEN) as *mut u8, length)
        };
        let result = f(packet);
        compiler_fence(Ordering::SeqCst);
        self.tx
            .set_descriptor(descriptor, buffer, (NET_HEADER_LEN + length) as u32, 0);
        self.tx.push_available(descriptor);
        self.mmio.notify(TX_QUEUE);
        result
    }
}

fn allocate_page() -> Result<usize, &'static str> {
    alloc_frame().map_err(|_| "out of physical memory for virtio-net")
}

pub struct VirtioNetRxToken {
    packet: Vec<u8>,
}

impl phy::RxToken for VirtioNetRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.packet)
    }
}

pub struct VirtioNetTxToken<'a> {
    driver: &'a mut VirtioNet,
    descriptor: Option<u16>,
}

impl phy::TxToken for VirtioNetTxToken<'_> {
    fn consume<R, F>(mut self, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let descriptor = self.descriptor.take().expect("virtio-net TX token reused");
        self.driver.transmit_reserved(descriptor, length, f)
    }
}

impl Drop for VirtioNetTxToken<'_> {
    fn drop(&mut self) {
        if let Some(descriptor) = self.descriptor {
            self.driver.release_tx(descriptor);
        }
    }
}

impl Device for VirtioNet {
    type RxToken<'a>
        = VirtioNetRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = VirtioNetTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.has_rx_packet() {
            return None;
        }
        let descriptor = self.reserve_tx()?;
        let packet = match self.poll_rx() {
            Some(packet) => packet,
            None => {
                self.release_tx(descriptor);
                return None;
            }
        };
        Some((
            VirtioNetRxToken { packet },
            VirtioNetTxToken {
                driver: self,
                descriptor: Some(descriptor),
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let descriptor = self.reserve_tx()?;
        Some(VirtioNetTxToken {
            driver: self,
            descriptor: Some(descriptor),
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = ETHERNET_MTU;
        capabilities.max_burst_size = Some(QUEUE_SIZE);
        capabilities
    }
}
