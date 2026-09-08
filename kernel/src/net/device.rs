//! smoltcp `Device` implementation over the virtio-net queues.
//!
//! `receive()` hands out an [`NetRxToken`] that owns a mutable borrow of the RX
//! queue and a [`NetTxToken`] that owns a mutable borrow of the TX queue; the
//! two are disjoint fields of [`Net`], so no raw pointers or interior
//! mutability are needed.  If smoltcp drops an RX token without consuming it,
//! `Drop` puts the buffer back on the available ring.

use super::virtio::{Net, RxQueue, TxQueue};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

/// Ethernet MTU including the 14-byte header, excluding the FCS.
const ETH_MTU: usize = 1514;

/// RX token: one received Ethernet frame.
pub struct NetRxToken<'a> {
    q: &'a mut RxQueue,
    id: u16,
    len: usize,
    /// Cleared by `consume` so `Drop` does not recycle twice.
    live: bool,
}

impl RxToken for NetRxToken<'_> {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.live = false;
        let r = f(self.q.frame_mut(self.id, self.len));
        self.q.recycle(self.id);
        r
    }
}

impl Drop for NetRxToken<'_> {
    fn drop(&mut self) {
        if self.live {
            self.q.recycle(self.id);
        }
    }
}

/// TX token: one descriptor-free slot on the TX ring.
pub struct NetTxToken<'a> {
    q: &'a mut TxQueue,
}

impl TxToken for NetTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.q.send(len, f)
    }
}

impl Device for Net {
    type RxToken<'a> = NetRxToken<'a>;
    type TxToken<'a> = NetTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Disjoint borrows of two fields of `self`.
        let (rx, tx) = (&mut self.rx, &mut self.tx);
        tx.reclaim();
        let (id, len) = rx.pop_used()?;
        rx.packets += 1;
        Some((
            NetRxToken {
                q: rx,
                id,
                len,
                live: true,
            },
            NetTxToken { q: tx },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        // Returning None when the ring is full leaves the packet queued in
        // smoltcp instead of dropping it.
        if !self.tx_ready() {
            return None;
        }
        Some(NetTxToken { q: &mut self.tx })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = ETH_MTU;
        caps.max_burst_size = None;
        // No checksum offload is negotiated, so smoltcp must do it in software.
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}
