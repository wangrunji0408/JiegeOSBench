#!/bin/zsh
# Build the kernel and run it under QEMU virt.
set -e
cd "$(dirname "$0")"
cargo build --release 2>&1 | grep -E "error|warning: unused|Finished" || true
K=target/riscv64gc-unknown-none-elf/release/kernel
rust-objcopy --strip-all "$K" -O binary "$K.bin"

QEMU_ARGS=(
  -machine virt
  -nographic
  -bios default
  -kernel "$K.bin"
  -m 1024M
  -smp 1
  -global virtio-mmio.force-legacy=false
  -netdev user,id=net0,hostfwd=tcp::8080-:80
  -device virtio-net-device,netdev=net0
)

exec qemu-system-riscv64 "${QEMU_ARGS[@]}" "$@"
