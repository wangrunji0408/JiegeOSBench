# `kernel/src/net` — virtio-net + smoltcp TCP/IP

Three source files:

| file | contents |
| --- | --- |
| `mod.rs` | the public API consumed by the syscall layer (handle types + 16 functions) |
| `virtio.rs` | virtio-mmio probe, feature negotiation, split virtqueues, RX/TX |
| `device.rs` | `smoltcp::phy::Device` glue (RX/TX tokens) |
| `stack.rs` | smoltcp `Interface`, socket table, TCP API implementation, `poll()` |

Dependency (in `kernel/Cargo.toml`):

```toml
smoltcp = { version = "0.11", default-features = false, features = ["medium-ethernet", "proto-ipv4", "socket-tcp", "alloc"] }
```

No `std`; buffers are `alloc::vec::Vec`-backed `SocketBuffer`s (`SocketSet<'static>`).

---

## 1. Device discovery

`net::init()` probes 8 virtio-mmio slots at `0x1000_1000 + 0x1000 * n`
(`SLOT_BASE`/`SLOT_STRIDE`/`MAX_SLOTS`). Every slot is validated on
`MagicValue == 0x74726976`, `VendorID == 0x554d4551`, `DeviceID == 1` (net).
MMIO is accessed through `mm::address::dev_addr()`; the device window
`0x1000_0000..0x1010_0000` is **not** identity mapped.

virtio-mmio slot *N* is wired to PLIC IRQ *N+1*; the handler is registered with
`plic::register(slot + 1, irq_handler)`.

### Both transports are supported — and this matters

On the reference launch line
(`-netdev user,id=n0,hostfwd=tcp::8080-:80 -device virtio-net-device,netdev=n0`)
QEMU's `virtio-mmio` defaults to **`force-legacy=on`**, so `Version` reads **1**
and the device is a *legacy* virtio device. With
`-global virtio-mmio.force-legacy=off` it reads **2** (modern virtio 1.x).
The driver accepts both:

| | legacy (MMIO v1) | modern (MMIO v2) |
| --- | --- | --- |
| queue addressing | `GuestPageSize` (0x028), `QueueAlign` (0x03c), `QueuePFN` (0x040) | `QueueDesc/Driver/Device Low+High` (0x080…0x0a4), `QueueReady` (0x044) |
| feature words | one 32-bit word | two words via `Device/DriverFeaturesSel` |
| `FEATURES_OK` | not used | required, verified after write |
| `VIRTIO_F_VERSION_1` | not offered | required (bit 32) |
| **virtio-net header** | **10 bytes** | **12 bytes** |

`Net::transport` and `Net::hdr_len` expose what was detected; `selftest()`
prints them.

### Register offsets used (`virtio.rs`)

```
0x000 MagicValue (0x74726976)      0x050 QueueNotify
0x004 Version    (1 or 2)          0x060 InterruptStatus
0x008 DeviceID   (1 = net)         0x064 InterruptACK
0x00c VendorID   (0x554d4551)      0x070 Status
0x010 DeviceFeatures               0x080 QueueDescLow   / 0x084 QueueDescHigh
0x014 DeviceFeaturesSel (modern)   0x090 QueueDriverLow / 0x094 QueueDriverHigh
0x020 DriverFeatures               0x0a0 QueueDeviceLow / 0x0a4 QueueDeviceHigh
0x024 DriverFeaturesSel (modern)   0x0fc ConfigGeneration
0x028 GuestPageSize (legacy)       0x100+ device config space (MAC at +0)
0x030 QueueSel     0x034 QueueNumMax
0x038 QueueNum     0x03c QueueAlign (legacy)  0x040 QueuePFN (legacy)
0x044 QueueReady (modern)
```

`QueueDriver*` is the **available** ring (driver-written), `QueueDevice*` is the
**used** ring (device-written).

## 2. Feature bits negotiated

* `VIRTIO_NET_F_MAC` (bit 5) — read the MAC from config space.
* `VIRTIO_F_VERSION_1` (bit 32) — **modern transport only**, mandatory.
* Everything else is rejected: in particular **no** `VIRTIO_NET_F_MRG_RXBUF`,
  no GSO/TSO/UFO, no checksum offload, no `VIRTIO_NET_F_STATUS`, no ctrl VQ.
  Checksums are computed/verified by smoltcp (`ChecksumCapabilities::default()`).

Status sequence: `ACKNOWLEDGE` → `ACKNOWLEDGE|DRIVER` → negotiate → (modern:
`|FEATURES_OK`, re-read to confirm) → queues → `|DRIVER_OK`.

## 3. Header size (the classic corruption trap)

```
struct virtio_net_hdr {          // 10 bytes, legacy, no MRG_RXBUF
    u8  flags; u8 gso_type;
    le16 hdr_len; le16 gso_size;
    le16 csum_start; le16 csum_offset;
};                               // + le16 num_buffers = 12 bytes
```

`num_buffers` is present **iff** either `VIRTIO_NET_F_MRG_RXBUF` is negotiated
(legacy rule) **or** `VIRTIO_F_VERSION_1` is negotiated (modern rule). QEMU's
`virtio_net_set_mrg_rx_bufs()` implements exactly this:

```c
if (version_1) n->guest_hdr_len = sizeof(struct virtio_net_hdr_mrg_rxbuf); // 12
else           n->guest_hdr_len = mergeable_rx_bufs ? 12 : 10;
```

So the header is **10 bytes on a legacy device and 12 bytes on a modern one**,
even though we never negotiate `MRG_RXBUF`. `HDR_LEGACY`/`HDR_MODERN` in
`virtio.rs` are used per device (`Vq::hdr_len`). On TX the driver zeroes the
whole header and sets `num_buffers = 1` only when `hdr_len >= 12`. On RX it
skips `hdr_len` bytes and hands `used.len - hdr_len` to smoltcp.

Both were verified at runtime under QEMU (see §8).

## 4. Virtqueue layout

One RX queue (index 0) and one TX queue (index 1), split virtqueues,
`QUEUE_NUM = 128` descriptors (QEMU's `QueueNumMax` is 256; the code clamps to
the smaller of 128 and the device's value, rounded down to a power of two).

Per queue two contiguous frame allocations from `mm::frame`:

* **ring area** — `alloc_frames()`; contains
  * descriptor table: `num * 16` bytes at offset 0 (16-byte aligned),
  * available ring at offset `num*16`: `flags: u16, idx: u16, ring[num]: u16`,
  * used ring at `used_off`: `flags: u16, idx: u16, ring[num]: {id: u32, len: u32}`.
* **buffer area** — `alloc_frames()`, `num * BUF_SIZE` bytes, buffer `i` at
  `buf_pa + i*2048`. RX buffers are device-writable (`VIRTQ_DESC_F_WRITE`),
  TX buffers are read-only for the device.

Concrete offsets with the defaults (`num = 128`, `BUF_SIZE = 2048`):

```
modern:  desc 0..2048   avail 2048..2310 (flags@2048 idx@2050 ring@2052)
         used 2312..3342 (flags@2312 idx@2314 ring@2316)      ring = 1 frame
legacy:  desc 0..2048   avail 2048..2310
         used 4096..5126 (ALIGN(desc+avail, 4096) = 4096)     ring = 2 frames
         QueuePFN = ring_pa / 4096, QueueAlign = 4096, GuestPageSize = 4096
```

Legacy layout follows `ALIGN(desc+avail) + ALIGN(used)` from virtio 1.1 §2.6.2
exactly, because the device reconstructs the addresses from `QueuePFN`.

Frame accounting per queue: RX/TX each take 1 ring frame (modern) or 2 (legacy)
plus `128 * 2048 / 4096 = 64` buffer frames → **~520 KiB total** for both queues.

### Descriptor bookkeeping

* **RX**: all descriptors are pre-filled and published once at init. After a
  packet is consumed the descriptor is rewritten and re-published on the
  available ring (`recycle`). `RxQueue::last_used` walks the used ring; the
  used index is re-read with a `fence(Acquire)`.
* **TX**: descriptors form a singly-linked free list (`Desc::next`,
  `free_head`, `num_free`). `send()` pops one, writes header+frame into that
  descriptor's fixed buffer, and publishes it. `reclaim()` walks the used ring
  and pushes finished descriptors back on the free list — called from
  `receive()`, `transmit()` and `tx_ready()`, so TX cannot stall.
* `transmit()` returns `None` when no descriptor is free, which makes smoltcp
  keep the packet queued rather than drop it.

## 5. Locking / interrupt model

Everything (device + `Interface` + `SocketSet` + handle table) lives in one
`SpinLock<Option<Stack>>` (`net::STACK`).

* `net::poll()` uses **`try_lock`**. If a thread is already inside, the
  interrupt handler returns immediately; the thread's own `poll()` drains the
  used rings, so no data is lost.
* The PLIC handler therefore **must** acknowledge `InterruptStatus`
  unconditionally, without taking the stack lock, otherwise the level-triggered
  PLIC line would immediately re-fire and livelock while a thread holds the
  lock. `virtio::Net::ack_interrupt_nolock()` does this using a cached MMIO base
  (`IRQ_BASE`).
* The public `tcp_*` calls use `lock()` and are meant for thread/syscall
  context; they must not be called from an interrupt handler.
* `tcp_accept()` calls `poll()` before taking the lock (API contract), so a
  caller looping with `sleep_ticks(1)` makes progress.

## 6. smoltcp configuration

```text
MAC           52:54:00:12:34:56 (from device config space, VIRTIO_NET_F_MAC)
IP            10.0.2.15/24        (slirp)
default route 10.0.2.2            (slirp gateway)
DNS           10.0.2.3            (constant exported, not used: no DNS socket)
MTU           1514 (Ethernet) -> 1500 IP
sockets       tcp::SocketBuffer 64 KiB per direction, ack_delay = None
```

## 7. Public API notes (for the syscall layer)

* `tcp_listen(port)` — `port == 0` picks an ephemeral port (Linux auto-bind).
* `tcp_accept(h)` — returns `Ok(None)` until a connection *completes*
  (`SynReceived` is not accepted yet); the returned handle is a fresh slot and
  the listen handle stays valid (a new listening socket is re-armed on the same
  port, because smoltcp 0.11 has no `accept()` that clones a connection).
* `tcp_state` on a listener returns `Listen`; `tcp_local_addr` on a listener
  returns `(10.0.2.15, bound_port)`; `tcp_peer_addr` returns `None`.
* `tcp_recv` — `Ok(0)` = EOF, `Err(Again)` = no data buffered, `Err(Closed)` =
  handle is not a connection.
* `tcp_drop` — a live connection is closed gracefully (FIN) and kept as a
  "zombie" slot until the handshake finishes; `poll()` then reaps it. The
  handle is invalid immediately. This is what makes an HTTP response followed by
  `close()` reach the peer intact instead of an RST.
* `tcp_shutdown(h, read, write)` — `write` sends FIN; `read` is a no-op
  (smoltcp 0.11 has no receive-half close).

## 8. Verification status

**Verified (compiles):** `cargo build` in the real tree: 0 errors and 0 warnings
from `src/net/`. While the rest of the tree was mid-refactor, `net` was also
compile-verified in a scratch copy with `task`/`syscall`/`socket` stubbed out
(`/tmp/netcheck2.sh`).

**Verified (runtime, QEMU 11.1.1, riscv64 `-M virt`, slirp):** the kernel boots,
probes slot 7, reads the MAC, brings the stack up and moves real bytes. Both
transports and both directions were exercised (scratch kernel in `/tmp/nv`,
workspace never modified):

| test | legacy (`transport=Legacy hdr=10`) | modern (`-global virtio-mmio.force-legacy=off`, `hdr=12`) |
| --- | --- | --- |
| outbound `tcp_connect` to host `10.0.2.2:8123`, HTTP GET | 300253 bytes, `HTTP/1.0 200 OK`, EOF | 300253 bytes, `HTTP/1.0 200 OK`, EOF |
| inbound `tcp_listen(80)` + host `curl` via hostfwd | `ACCEPT 10.0.2.2:54303`, request 78 B, response 55 B, `HTTP 200` | same, `HTTP 200`, body `ijiege-listen-ok` |
| PLIC interrupt delivery (`sstatus.SIE` on) | `irqs=8` for a 6-packet connection | — |

The outbound run streams ~200 full-size TCP segments, so RX segmentation,
TX descriptor reclamation and EOF handling are all exercised. `selftest()`
after a connection prints, e.g.:

```
[net] selftest: present=true irqs=8 mac=52:54:00:12:34:56 ip=10.0.2.15
[net]   device: slot=7 base=0x10008000 transport=Legacy hdr_len=10 status=0x7 features=0x39bf8064 irq_status=0x1
[net]   rx: num=128 used_idx=6 last_used=6 packets=6 malformed=0
[net]   tx: num=128 free=127 used_idx=6 last_used=5 packets=6 dropped=0
[net]   sockets: 2 handles (1 listening, 0 connected, 1 zombie)
```

**Not verified at runtime:** IPv4 fragmentation/reassembly (not enabled in
smoltcp), UDP/ICMP/DNS, multi-hart, behaviour when the 64 MiB kernel heap is
exhausted, and end-to-end traffic through the *integrated* kernel (the real
kernel was booted under QEMU and initialised the NIC, but with no initramfs
there is no process to drive traffic; the traffic tests ran on a scratch build
of the same `net` code).

## 9. Debugging checklist

1. `net::selftest()` (never called from `init`) prints transport, header size,
   device status, feature words, IRQ status, ring indices, free TX descriptors
   and per-queue counters.
2. `present() == false` → slot not found. Check `MagicValue`/`VendorID`/
   `DeviceID`/`Version` at `dev_addr(0x1000_1000 + 0x1000*n)`. QEMU with
   `-device virtio-net-device,netdev=n0` puts the NIC in the **highest** slot.
3. Packets received but garbage → wrong header size. Legacy = 10, modern = 12.
4. TX stalls → `tx.free == 0` means `reclaim()` is not being called (it is
   called from `poll()`/`receive()`/`transmit()`).
5. Interrupt storm / hang → `InterruptStatus` must be ACKed in the handler
   without the stack lock.
