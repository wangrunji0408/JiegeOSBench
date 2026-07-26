//! Networking: a smoltcp-based TCP/IP stack over virtio-net.

pub mod addr;
pub mod socket;
pub mod stack;

pub use stack::{on_interrupt, poll};

/// Bring up the network stack. Requires the virtio-net driver to be initialized.
pub fn init() {
    stack::init();
}
