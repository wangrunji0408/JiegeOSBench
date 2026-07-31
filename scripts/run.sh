#!/bin/sh
# Run JiegeOS in QEMU. Host port 8080 is forwarded to guest port 80.
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="$ROOT/kernel/target/riscv64gc-unknown-none-elf/release/jiegeos-kernel"
MEM="${MEM:-512M}"

if [ ! -f "$KERNEL" ]; then
    echo "kernel not built: $KERNEL" >&2
    exit 1
fi

exec qemu-system-riscv64 \
    -machine virt \
    -m "$MEM" \
    -smp 1 \
    -bios default \
    -kernel "$KERNEL" \
    -nographic \
    -netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 \
    -device virtio-net-device,netdev=n0 \
    "$@"
