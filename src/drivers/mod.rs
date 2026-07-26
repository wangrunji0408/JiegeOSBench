//! Device drivers.

pub mod plic;
pub mod virtio;
pub mod virtio_net;

/// Probe the QEMU `virt` machine's MMIO regions for devices we support.
pub fn init() {
    plic::init();
    virtio::probe();
}
