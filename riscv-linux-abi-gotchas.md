---
name: riscv-linux-abi-gotchas
description: Non-obvious Linux/riscv64 ABI and smoltcp facts that silently break userspace — epoll_event is not packed, edge-triggered epoll needs consumption tracking, smoltcp can_recv lies after FIN
metadata:
  type: reference
---

Verified the hard way while making unmodified nginx run on a custom riscv64
kernel (see [[jiege-kernel-project]]). Each of these produces a symptom far from
its cause.

- **`struct epoll_event` is packed only on x86.** On riscv64 (and every other
  arch) it is naturally aligned: `data` at offset 8, size 16. Using
  `repr(C, packed)` truncates every pointer userspace stores in `data`, which
  surfaces as a segfault deep inside the application rather than a bad syscall.
  Confirm from the app's own disassembly: `slli aN, idx, 0x4` means stride 16.

- **Edge-triggered epoll cannot be implemented by polling alone.** Between a
  reader draining a socket and the next byte arriving, nothing observes the
  not-ready state in between, so a remembered `EPOLLIN` suppresses the new
  arrival forever. The kernel must count *consuming reads* (a generation counter
  on the open file description) and treat a bump as re-arming the watch.
  Symptom: every keep-alive connection stalls after exactly one request, while
  the kernel has already ACKed the bytes at TCP level.

- **`EPOLLHUP` means both directions are dead, `EPOLLRDHUP` means the peer sent
  FIN.** Reporting HUP for a half-closed (`CloseWait`) connection tells a server
  the connection is unusable when it can still write the response.

- **smoltcp's `tcp::Socket::can_recv()` is gated on `may_recv()`**, which goes
  false in `CloseWait` — so buffered, still-readable data becomes invisible once
  the peer sends FIN. Gate readiness and reads on `recv_queue() > 0` instead.
  `recv_slice` still returns the data.

- **A virtio 1.0 net header is 12 bytes** (`num_buffers` present regardless of
  `VIRTIO_NET_F_MRG_RXBUF`), and QEMU's `virt` machine defaults to *legacy*
  virtio-mmio — pass `-global virtio-mmio.force-legacy=false` for version 2.

- **A virtqueue descriptor chain must be unlinked from the free list before its
  `next` fields are overwritten**, or `add` corrupts the list it is walking.

- **`brk` must resize the heap VMA in place.** Re-mapping the range unmaps it
  first and discards everything the program allocated.

- **The ELF loader must not read past `p_filesz`** when populating a segment's
  last page; those bytes belong to the zero-filled tail, and reading them
  splices the next section (usually `.riscv.attributes`) into `.bss`.

Debugging technique that actually found these: `-object
filter-dump,id=d,netdev=netN,file=out.pcap` on QEMU plus a syscall trace, read
side by side. The pcap shows whether the kernel ACKed data it never delivered,
which immediately separates network bugs from delivery bugs.

Environment note: this machine has `http_proxy`/`all_proxy` set to a local proxy,
so `curl` against a QEMU hostfwd port returns a misleading `502 Bad Gateway`
unless invoked with `--noproxy '*'`.
