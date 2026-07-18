#!/bin/sh
# Build the kernel and boot it in QEMU with virtio-net + usermode
# networking, forwarding host port 8080 to the guest's port 80 (nginx).
#
# Usage: ./run.sh [extra qemu args...]
# Then, from another terminal: curl http://127.0.0.1:8080/

set -e
cd "$(dirname "$0")"

cargo build --release -p kernel

exec qemu-system-riscv64 \
    -machine virt \
    -m 512M \
    -nographic \
    -bios default \
    -kernel target/riscv64gc-unknown-none-elf/release/kernel \
    -netdev user,id=n0,hostfwd=tcp::8080-:80 \
    -device virtio-net-device,netdev=n0 \
    "$@"
