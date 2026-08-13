# iJiege Kernel

A RISC-V 64-bit operating system kernel written from scratch in Rust, capable of
running the **official nginx binary** and serving a website from QEMU, reachable
from the host.

```
Host (curl http://127.0.0.1:8080/)  ──►  QEMU (virt, riscv64)  ──►  nginx (static musl)  ──►  /www/index.html
                                          └─ iJiege kernel (Sv39, SBI, virtio-net + smoltcp)
```

## What it does

- Boots on QEMU's `virt` machine via OpenSBI (S-mode, Sv39 paging).
- Implements the Linux riscv64 syscall ABI and enough of the system-call surface
  for a statically-linked musl nginx to start, listen, and serve.
- Provides: memory management (frame allocator + Sv39 page tables + heap),
  process/ELF loading, an in-memory filesystem (ramfs), and a TCP/IP stack
  (smoltcp over a hand-written legacy virtio-net MMIO driver).
- Runs the **unmodified official nginx** (built from the nginx.org 1.30.4
  source tarball as a static riscv64 musl binary via `zig cc`).

## Requirements

- Rust nightly (for `alloc_error_handler`), `riscv64gc-unknown-none-elf` target
- QEMU (`qemu-system-riscv64`), OpenSBI (QEMU's `-bios default`)
- `zig` (only needed to rebuild nginx)

## Build & run

```sh
cargo build --release          # debug: cargo build
./scripts/run.sh               # boots the kernel + nginx in QEMU
```

`scripts/run.sh` forwards host TCP port `8080` to the guest's port `80`
(`-netdev user,hostfwd=tcp::8080-:80`). To change memory/ports, override
`MEM`, `PORTFWD`, etc.

## Test

```sh
curl --noproxy '*' http://127.0.0.1:8080/
```

Expected output:

```html
<!DOCTYPE html>
<html>
<head><title>nginx on iJiege</title></head>
<body>
<h1>Hello from nginx on my Rust RISC-V kernel</h1>
</body>
</html>
```

## Layout

- `src/` — kernel source
  - `main.rs`, `trap.rs`, `sbi.rs`, `console.rs`, `sync.rs`, `lang.rs`
  - `memory/` — frame allocator, Sv39 page tables, heap
  - `process/` — process management + ELF loader
  - `syscall/` — Linux riscv64 syscall dispatch
  - `fs.rs` — in-memory filesystem (ramfs)
  - `net/` — virtio-net driver + smoltcp integration + sockets
- `nginx-conf/`, `webroot/` — nginx config and site content (embedded into the fs)
- `third_party/nginx` — the static riscv64 nginx binary (rebuilt from
  `third_party/nginx-1.30.4`)

## Rebuilding nginx

The static nginx binary is produced from the official source with a minimal,
dependency-free configure (`--without-http_rewrite_module
--without-http_gzip_module --without-http_ssl_module`, etc.) using `zig cc`
as the riscv64-linux-musl cross compiler:

```sh
cd third_party/nginx-1.30.4
./configure --crossbuild=Linux:riscv64 \
    --with-cc="$(pwd)/../zigcc" --with-cc-opt="-static -Wno-sign-compare" \
    --with-ld-opt="-static" --prefix=/usr/local/nginx \
    --without-http_rewrite_module --without-http_gzip_module \
    --without-http_fastcgi_module --without-http_uwsgi_module \
    --without-http_scgi_module --without-http_memcached_module \
    --without-http_grpc_module --without-http_proxy_module
make -j$(sysctl -n hw.ncpu)
```

The `nginx.conf` uses `master_process off; daemon off;` so nginx runs as a
single foreground process (no fork/clone required by the kernel for this demo).
