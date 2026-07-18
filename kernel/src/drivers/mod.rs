pub mod virtio_hal;

use crate::config::MMIO;
use core::ptr::NonNull;
use virtio_drivers::transport::mmio::{MmioTransport, VirtIOHeader};
use virtio_drivers::transport::{DeviceType, Transport};

/// Probe the QEMU virt machine's virtio-mmio slots for a network device.
pub fn probe_net_transport() -> Option<MmioTransport> {
    let (base, len) = MMIO[0];
    let slot_size = 0x1000;
    let mut addr = base;
    while addr < base + len {
        let header = NonNull::new(addr as *mut VirtIOHeader).unwrap();
        if let Ok(transport) = unsafe { MmioTransport::new(header) } {
            if transport.device_type() == DeviceType::Network {
                return Some(transport);
            }
        }
        addr += slot_size;
    }
    None
}
