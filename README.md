# JiegeOS — a from-scratch RISC-V OS kernel running official nginx

JiegeOS is a from-scratch operating system kernel written in Rust for the
RISC-V `virt` machine under QEMU (S-mode, Sv39 paging). It boots with
OpenSBI, drives a virtio-net NIC, implements a small TCP/IP stack with
epoll, and runs the **unmodified official nginx 1.30.4** (statically linked
with musl for riscv64) as its init process — a real website is served from
the guest to the host through QEMU user networking.

```
$ curl http://127.0.0.1:8080/
<!DOCTYPE html>
<html>
<head><title>JiegeOS nginx</title></head>
...
```

## Components

| Path | What it is |
|---|---|
| `kernel/` | The Rust kernel: boot, Sv39 paging, frame + heap allocators, tasks/scheduler, syscalls (incl. epoll, signal), virtio-net driver, ARP/IP/ICMP/TCP, filesystem from an embedded initramfs (cpio) |
| `nginx/` | Official nginx-1.30.4 source tarball + build tree |
| `tools/` | `riscv64-musl-cc` / `riscv64-musl-ar`: zig-cc wrappers for the cross build |
| `initramfs/` | Build area + generated `initramfs.cpio` (nginx binary, config, html, /etc) |
| `scripts/` | `build-nginx.sh`, `build-initramfs.sh`, `run.sh` |

## Requirements

* macOS (arm64) or Linux; `zig` 0.16+ on PATH
* Rust **nightly** with the `riscv64gc-unknown-none-elf` target
* QEMU (`qemu-system-riscv64`) 7+ with user networking

## Build

```sh
# 1. nginx (official 1.30.4, static musl riscv64, no source changes)
./scripts/build-nginx.sh

# 2. initramfs (embeds the nginx binary + config + html)
./scripts/build-initramfs.sh nginx

# 3. kernel (embeds the initramfs)
cd kernel
cargo clean -p jiegeos-kernel   # required: stale fingerprints otherwise
cargo build --release
```

## Run

```sh
./scripts/run.sh                # QEMU virt, hostfwd 127.0.0.1:8080 -> guest :80
# in another terminal:
curl http://127.0.0.1:8080/
```

## Kernel features (highlights)

* Boot: OpenSBI → `_start` (0x80200000) → `rust_main`; 128 KiB boot stack;
  DTB parse for RAM / timebase / bootargs.
* Memory: 64 MiB first-fit heap with coalescing (8-byte aligned block
  boundaries), frame allocator, Sv39 with 2 MiB huge pages for RAM +
  4 KiB pages for MMIO; user VA space below 0x4000_0000_0000.
* Tasks: fork/clone (copy-on-write-free, eager page copy), exec, exit,
  wait4, signals (sigaction/sigreturn/sigprocmask, sigframe on user stack,
  Linux bit N-1 pending convention), per-task kernel + idle stacks,
  non-preemptible kernel.
* Syscalls: openat/read/write/close/lseek/fstat/newfstatat/getdents64/
  readlinkat, mmap/munmap/mprotect/brk/mremap, clone/execve/wait4/kill,
  rt_sig*, epoll_create1/epoll_ctl/epoll_pwait, eventfd2, socket/bind/
  listen/accept/connect/readv/writev/sendfile, setsockopt, ioctl,
  uname, prctl, getrlimit, sched_getaffinity, getgroups/setgroups,
  setuid/setgid, pread64/pwrite64, pipe2, dup/dup2/dup3, fcntl, ...
* Network: legacy virtio-net (virtio 0.9.5 ring), ARP cache/announce,
  IPv4 (checksum, fragment-free), ICMP echo, TCP state machine with
  retransmission, TIME_WAIT, epoll-driven sockets; guest IP 10.0.2.15,
  gateway 10.0.2.2 (QEMU slirp).

## Debugging aids (built in)

* Kernel page-fault handler dumps registers, the offending VA translation
  and the current task's VMAs before panicking.
* `[heap] FREE LIST CORRUPT` guard catches allocator/free-list corruption.
* QMP socket at `/tmp/qmp.sock` when launched with
  `-qmp unix:/tmp/qmp.sock,server,nowait` for register/memory inspection.

## Reproducibility

All sources, scripts and build outputs live in this directory. `make clean`
removes `nginx/nginx-1.30.4/objs` (the Makefile lives there); re-running
`scripts/build-nginx.sh` regenerates it. The kernel's `INITRAMFS`
(`include_bytes!`) must be rebuilt after any initramfs change, and the
kernel must be rebuilt with `cargo clean -p jiegeos-kernel` because stale
fingerprints otherwise leave an old binary in place.
