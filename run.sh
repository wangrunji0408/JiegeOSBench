#!/bin/bash
# 构建并运行 iJiege-k3 内核（RISC-V Rust 内核 + 官方 nginx）
cd "$(dirname "$0")"
cargo build --release || exit 1
exec qemu-system-riscv64 \
    -machine virt \
    -m 256M \
    -nographic \
    -bios default \
    -kernel target/riscv64gc-unknown-none-elf/release/ijiege-kernel \
    -global virtio-mmio.force-legacy=false \
    -netdev user,id=n0,hostfwd=tcp::8080-:80 \
    -device virtio-net-device,netdev=n0 \
    "$@"
