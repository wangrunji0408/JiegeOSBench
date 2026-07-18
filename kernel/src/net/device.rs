use super::VirtioNetImpl;
use smoltcp::phy::{self, Checksum, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

pub struct NetDevice {
    pub inner: VirtioNetImpl,
}

pub struct RxToken {
    buf: virtio_drivers::device::net::RxBuffer,
    dev: *mut NetDevice,
}

pub struct TxToken {
    dev: *mut NetDevice,
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = self.buf;
        let result = f(buf.packet_mut());
        unsafe {
            (*self.dev).inner.recycle_rx_buffer(buf).ok();
        }
        result
    }
}

impl phy::TxToken for TxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let dev = unsafe { &mut *self.dev };
        let mut tx_buf = dev.inner.new_tx_buffer(len);
        let result = f(tx_buf.packet_mut());
        dev.inner.send(tx_buf).ok();
        result
    }
}

impl phy::Device for NetDevice {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(RxToken, TxToken)> {
        if !self.inner.can_recv() {
            return None;
        }
        let buf = self.inner.receive().ok()?;
        let self_ptr: *mut NetDevice = self;
        Some((
            RxToken { buf, dev: self_ptr },
            TxToken { dev: self_ptr },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<TxToken> {
        if !self.inner.can_send() {
            return None;
        }
        Some(TxToken { dev: self })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1536;
        caps.medium = Medium::Ethernet;
        caps.checksum.ipv4 = Checksum::Tx;
        caps.checksum.tcp = Checksum::Tx;
        caps.checksum.udp = Checksum::Tx;
        caps.checksum.icmpv4 = Checksum::Tx;
        caps
    }
}
