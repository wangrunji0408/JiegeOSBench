#!/bin/bash
# Run the kernel in QEMU (RISC-V virt machine).
set -e
cd "$(dirname "$0")/.."

KERNEL="${KERNEL:-target/riscv64gc-unknown-none-elf/debug/ijiege-kernel}"
MEM="${MEM:-128M}"
SMP="${SMP:-1}"
PORTFWD="${PORTFWD:-hostfwd=tcp::8080-:80}"

exec qemu-system-riscv64 \
    -machine virt \
    -nographic \
    -bios default \
    -kernel "$KERNEL" \
    -m "$MEM" \
    -smp "$SMP" \
    -netdev user,id=net0,$PORTFWD \
    -device virtio-net-device,netdev=net0 \
    "$@"
