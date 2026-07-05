# JiegeOS

A RISC-V OS kernel written in Rust **from scratch** that boots on QEMU and runs the
**official, unmodified nginx binary** (Alpine Linux riscv64 package, `nginx/1.28.3`),
serving a website reachable from the host.

```
host: curl http://127.0.0.1:8080/   →   QEMU virt (riscv64)
                                          └─ JiegeOS kernel (Rust, S-mode)
                                               └─ /usr/sbin/nginx  (unmodified ELF)
                                                    └─ ld-musl-riscv64.so.1 + libssl/libcrypto/libpcre2/libz
```

## Quick start

```bash
./run.sh                        # builds rootfs.tar + kernel, boots QEMU
# in another terminal:
curl http://127.0.0.1:8080/
```

Requirements: Rust (target `riscv64gc-unknown-none-elf`), `qemu-system-riscv64`.

## How it works

**Boot & CPU** — OpenSBI (QEMU `-bios default`) drops us at `0x8020_0000` in S-mode.
The kernel runs a *single hart, zero interrupts* design: `sie = 0`, all device I/O is
polled. There is exactly one user process (nginx with `master_process off`), so no
scheduler is needed — blocking syscalls poll the NIC in a loop.

**Memory** — Sv39 paging. Each address space is three 1 GiB slots:
`root[0]` = MMIO identity map (UART/virtio, kernel-only), `root[1]` = user space
(0x4000_0000..0x8000_0000: ELF, interpreter, brk, mmap arena, stack),
`root[2]` = one 1 GiB identity megapage for kernel RAM. The kernel never switches
page tables on trap; with `sstatus.SUM` it dereferences user pointers directly.

**Linux ABI** — the kernel implements ~70 syscalls of the riscv64 Linux ABI —
enough for musl's dynamic linker and nginx: `mmap/mprotect/brk`, `openat/read/
writev/pread64/fstatat`, `epoll_create1/ctl/pwait`, `socket/bind/listen/accept4/
recvfrom/sendmsg`, `ioctl(FIONBIO)`, `fcntl`, signals (recorded, never delivered),
`futex` (single-threaded stub), `clock_gettime` via `rdtime`, etc.

**ELF loading** — loads PIE executables plus their `PT_INTERP` interpreter
(`ld-musl-riscv64.so.1`), builds the SysV stack (argv/envp/auxv with
`AT_PHDR/AT_BASE/AT_RANDOM/...`), and jumps to the interpreter's entry.
The dynamic linker then mmaps the real shared libraries itself.

**Filesystem** — a `rootfs.tar` (ustar) is embedded into the kernel image at build
time and unpacked into an in-memory ramfs at boot. It contains the Alpine nginx
binary, musl, libpcre2/libssl/libcrypto/libz, `nginx.conf`, and the web root.

**Network** — a polled legacy virtio-mmio net driver feeds
[smoltcp](https://github.com/smoltcp-rs/smoltcp) (TCP/IP). BSD sockets are mapped
onto smoltcp: `listen()` creates a pool of smoltcp listening sockets (backlog),
`accept4()` harvests established ones and replenishes the pool. epoll is
level-triggered readiness over socket state. QEMU user-mode networking forwards
host `127.0.0.1:8080` → guest `10.0.2.15:80`.

## Layout

```
src/main.rs        boot, user-mode run loop
src/trap.rs        trap entry/exit assembly, TrapFrame
src/mm/            frame allocator, kernel heap, Sv39 page tables
src/loader.rs      ELF64 + PT_INTERP loader, stack/auxv setup
src/fs.rs          tar-backed ramfs, fd table
src/task.rs        the (single) task: address space, fds
src/syscall/       Linux syscall implementations (fs, mm, proc, time, net)
src/net/           virtio-net driver + smoltcp glue + sockets + epoll
rootfs/            guest filesystem source (Alpine riscv64 packages, config, html)
mkfs.sh            packs rootfs/ → build/rootfs.tar
run.sh             build + boot QEMU with port forward
```

## Verified

- `curl http://127.0.0.1:8080/` returns the page, `Server: nginx/1.28.3`
- 100 sequential + 200 concurrent + 30 s sustained load (6607 requests): 100 % OK
- 2 MiB file download: byte-identical (md5 verified)
- HTTP keep-alive, HEAD, 404 paths work
