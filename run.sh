#!/bin/bash
# Build and run JiegeOS in QEMU with nginx port forwarded to host :8080.
set -e
cd "$(dirname "$0")"
./mkfs.sh > /dev/null
cargo build --release
exec qemu-system-riscv64 \
    -machine virt \
    -smp 1 -m 1024M \
    -nographic \
    -bios default \
    -kernel target/riscv64gc-unknown-none-elf/release/jiege-os \
    -netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 \
    -device virtio-net-device,netdev=n0 \
    "$@"
