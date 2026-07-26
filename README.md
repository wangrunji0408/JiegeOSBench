# 智能杰哥 · jiege-kernel

A RISC-V (rv64gc) operating system kernel written from scratch in Rust, built to
run the **official, unmodified nginx binary** for riscv64 and serve HTTP to the
outside world from inside QEMU.

```
$ curl http://localhost:8080/
HTTP/1.1 200 OK
Server: nginx/1.30.4
...
```

The nginx binary is the Alpine Linux `nginx-1.30.4-r2` riscv64 package, taken as
published. It is a dynamically linked PIE against musl, so the kernel has to
implement enough of Linux for `ld-musl-riscv64.so.1` to relocate it, for nginx to
fork a worker, drop privileges, and drive an epoll event loop over BSD sockets.

## Quick start

```sh
make            # build the rootfs image and the kernel
make run        # boot it in QEMU with host port 8080 forwarded to guest port 80
curl http://localhost:8080/
```

Requirements: a Rust nightly toolchain with the `riscv64gc-unknown-none-elf`
target, `qemu-system-riscv64` (v7+), and `curl` to fetch the Alpine packages.

## What it does

| Layer | Implementation |
|---|---|
| Boot | SBI (OpenSBI) console and timer, single hart |
| Memory | Sv39 paging, buddy kernel heap, refcounted frame allocator, lazy page population, copy-on-write `fork` |
| Traps | Full U↔S trap path with FPU save/restore, PLIC external interrupts, preemptive 100 Hz timer |
| Processes | Threads and thread groups, round-robin scheduler, `clone`/`fork`/`execve`/`wait4`, futexes |
| Signals | Real delivery: signal frames on the user stack, `sigaction`, masks, `sigaltstack`, `rt_sigreturn` |
| Files | VFS over a writable ramfs, devfs, pipes, a generated `/proc` and `/sys`, epoll and eventfd |
| Loading | ELF64 loader with `PT_INTERP` support and a complete auxiliary vector |
| Network | virtio-net (modern virtio 1.0 MMIO) driver, smoltcp TCP/IP, BSD socket syscalls |

Roughly 170 Linux syscalls are implemented.

## Architecture notes

**Address space layout.** The kernel identity-maps the low 4 GiB with gigapages
and places all user mappings above `0x1_0000_0000`. Every address space therefore
contains both, and with `sstatus.SUM` set the kernel dereferences user pointers
directly — no temporary mappings, and `uaccess` only has to fault pages in first.

**Listen backlog.** smoltcp has no listening backlog: a socket in `Listen` state
*becomes* the connection when a SYN arrives. To present a POSIX listening socket
that can be accepted from repeatedly, a listener owns a pool of smoltcp sockets
all parked on the same endpoint; `accept` takes one that has connected and spawns
a replacement, keeping the backlog filled.

**IPv6.** The stack is IPv4-only, but nginx configures `listen [::]:80` next to
`listen 80` and treats a failure on either as fatal. An `AF_INET6` socket
therefore binds and listens successfully but stays *inert* — it never receives a
connection, exactly as a dual-stack kernel with `IPV6_V6ONLY` behaves. Letting it
share the port with a live listener instead would silently swallow connections
that nobody ever accepts.

## Bugs worth remembering

Four of these cost real debugging time, and each has a comment at the site:

1. **`epoll_event` is not packed on riscv64.** Only x86 packs it. Getting it
   wrong truncates every pointer nginx stores in `data`, surfacing as a segfault
   deep in its event loop rather than as a bad syscall.

2. **Edge-triggered epoll needs to know when data was *consumed*.** Polling alone
   cannot distinguish a fresh arrival from data already reported and never read —
   between nginx reading request N and request N+1 arriving, nothing observes the
   not-ready state in between, so a remembered `EPOLLIN` suppresses the new
   arrival forever. `File::read_generation` counts consuming reads and re-arms
   the watch. Without it, every keep-alive connection stalls after one request.

3. **smoltcp's `can_recv()` lies once the peer sends FIN.** It is gated on
   `may_recv()`, which goes false in `CloseWait` — so buffered, still-readable
   data becomes invisible. The readiness and receive paths gate on `recv_queue()`
   instead.

4. **`brk` must resize the heap VMA, not re-map it.** The obvious
   `map_region(start, new_end, ...)` unmaps the old range first and throws away
   everything the program has allocated. `AddrSpace::resize_vma` grows in place.

Also: the ELF loader must not read past `p_filesz` into the following section
when populating the last page of a segment, and a `virtio` descriptor chain must
be unlinked from the free list *before* its `next` fields are overwritten.

## Measured behaviour

On an M-series Mac under QEMU TCG:

```
keep-alive, one connection        3000/3000 requests   ~1185 req/s
fresh connection per request       320/320 connections  ~726 conn/s
concurrent connections              50/50              all 200
```

Verified: correct status codes (200/404), `HEAD`, `ETag` with conditional `304`,
and byte-exact response bodies.

## Layout

```
src/
  main.rs        boot, init order, panic handler
  entry.rs       assembly entry, boot stack
  arch.rs        CSR access
  sbi.rs         SBI calls
  trap/          trap entry (trap.S), contexts, dispatch
  mm/            frame allocator, heap, Sv39 page tables, address spaces, uaccess
  task/          tasks, scheduler, futexes
  signal.rs      signal delivery and return
  fs/            VFS, ramfs, devfs, pipes, procfs, tar extractor
  loader.rs      ELF loading, stack and auxv setup
  drivers/       PLIC, virtio-mmio transport, virtio-net
  net/           smoltcp integration, sockets, address conversion
  syscall/       syscall table and handlers, grouped by area
scripts/
  build-rootfs.sh  fetch Alpine packages, build build/rootfs.tar
  run.sh           launch QEMU with networking
```

The rootfs is embedded in the kernel image with `include_bytes!` and unpacked
into the ramfs at boot, so there is no block device or disk image to manage.

Set `JIEGE_TRACE=1` at build time for a syscall trace and periodic health lines.
