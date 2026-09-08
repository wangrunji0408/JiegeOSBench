//! Networking: virtio-net driver + smoltcp-based TCP/IP + socket layer.
//!
//! Public API is consumed by the syscall layer; see `README.md` in this
//! directory for the virtio register/queue details and the header size.

pub mod stack;
pub mod virtio;

mod device;

use crate::sync::SpinLock;

/// Guest IPv4 address on QEMU slirp user networking.
pub const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
/// slirp gateway.
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
/// slirp DNS.
pub const DNS_IP: [u8; 4] = [10, 0, 2, 3];
/// MAC expected by the reference QEMU launch line; overridden by the MAC the
/// device reports when `VIRTIO_NET_F_MAC` is negotiated.
pub const GUEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

/// Error kind returned by the socket calls (mirrors Linux errno classes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrnoKind {
    Again,
    Closed,
    Invalid,
}

/// TCP connection state, mirroring smoltcp/Linux.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

/// A TCP socket handle. Handles are indices into a global handle table; they
/// stay valid until `tcp_drop`.
#[derive(Clone, Copy, Debug)]
pub struct TcpHandle(pub usize);

/// The whole stack behind one lock. `poll()` may run in interrupt context and
/// therefore only ever uses `try_lock`.
pub(crate) static STACK: SpinLock<Option<stack::Stack>> = SpinLock::new(None);

/// Probe virtio-mmio slots 0..8 for a net device, set up queues, install the
/// PLIC irq handler and bring the smoltcp interface up.  Never panics when no
/// device is present.
pub fn init() {
    stack::init();
}

/// Pump RX/TX and smoltcp timers. Safe to call from interrupt context and from
/// threads; cheap and reentrancy-safe.
pub fn poll() {
    stack::poll();
}

/// Device found and stack ready.
pub fn present() -> bool {
    stack::present()
}

/// Our IPv4 address.
pub fn ip() -> [u8; 4] {
    GUEST_IP
}

/// Our MAC address (the device's, once probed).
pub fn mac() -> [u8; 6] {
    stack::mac()
}

/// Create a listening socket.  Port 0 picks an ephemeral port.
pub fn tcp_listen(port: u16) -> Result<TcpHandle, &'static str> {
    stack::tcp_listen(port)
}

/// Return a completed connection, if any, as a NEW connected socket.  Pumps the
/// stack internally, so callers may loop with a short sleep.
pub fn tcp_accept(h: TcpHandle) -> Result<Option<(TcpHandle, [u8; 4], u16)>, &'static str> {
    stack::tcp_accept(h)
}

/// True when the listen pool for this handle has an acceptable connection.
pub fn tcp_listen_pending(h: TcpHandle) -> bool {
    stack::tcp_listen_pending(h)
}

/// Start a non-blocking connect.  Poll `tcp_state` until `Established`.
pub fn tcp_connect(ip: [u8; 4], port: u16) -> Result<TcpHandle, &'static str> {
    stack::tcp_connect(ip, port)
}

/// Queue bytes for transmission.  `ErrnoKind::Again` means it would block,
/// `ErrnoKind::Closed` means the connection is closed or reset.
pub fn tcp_send(h: TcpHandle, buf: &[u8]) -> Result<usize, ErrnoKind> {
    stack::tcp_send(h, buf)
}

/// Read received bytes.  `Ok(0)` means orderly closed (EOF); `Again` means no
/// data is buffered yet.
pub fn tcp_recv(h: TcpHandle, buf: &mut [u8]) -> Result<usize, ErrnoKind> {
    stack::tcp_recv(h, buf)
}

/// Current TCP state of the handle.
pub fn tcp_state(h: TcpHandle) -> TcpState {
    stack::tcp_state(h)
}

/// Local (ip, port) of the connection, or the bound port of a listener.
pub fn tcp_local_addr(h: TcpHandle) -> Option<([u8; 4], u16)> {
    stack::tcp_local_addr(h)
}

/// Remote (ip, port) of the connection.
pub fn tcp_peer_addr(h: TcpHandle) -> Option<([u8; 4], u16)> {
    stack::tcp_peer_addr(h)
}

/// Half-close: `write` sends FIN.  `read` is accepted but has no effect
/// (smoltcp 0.11 has no receive-half close); see `stack::tcp_shutdown`.
pub fn tcp_shutdown(h: TcpHandle, read: bool, write: bool) {
    stack::tcp_shutdown(h, read, write)
}

/// Release a handle.  A connected socket is closed gracefully and reaped by
/// `poll()` once the FIN handshake finishes.
pub fn tcp_drop(h: TcpHandle) {
    stack::tcp_drop(h)
}

/// Bytes available to read.
pub fn tcp_pending(h: TcpHandle) -> usize {
    stack::tcp_pending(h)
}

/// Print device/stack status.  Deliberately not called from `init`.
pub fn selftest() {
    stack::selftest();
}
