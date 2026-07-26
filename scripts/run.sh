#!/bin/bash
# Boot the kernel in QEMU with a virtio-net device, forwarding host port 8080 to
# the guest's port 80 so the nginx running inside is reachable from outside.
set -euo pipefail

cd "$(dirname "$0")/.."

KERNEL=target/riscv64gc-unknown-none-elf/release/jiege-kernel
HOST_PORT="${HOST_PORT:-8080}"

# `-nographic` puts the serial console on stdout.
exec qemu-system-riscv64 \
    -machine virt \
    -cpu rv64 \
    -smp 1 \
    -m 1G \
    -nographic \
    -bios default \
    -kernel "$KERNEL" \
    -netdev "user,id=net0,hostfwd=tcp::${HOST_PORT}-:80" \
    -device virtio-net-device,netdev=net0 \
    "$@"
