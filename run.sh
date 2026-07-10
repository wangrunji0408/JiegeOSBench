#!/bin/bash
set -e

KERNEL_DIR="kernel"
KERNEL_ELF="$KERNEL_DIR/target/riscv64gc-unknown-none-elf/release/jiege-os"
INITRAMFS="initramfs.cpio"

# Build kernel
cd "$KERNEL_DIR"
cargo build --release 2>&1 | grep -E "^error|^warning.*jiege|Compiling|Finished" || true
cd ..

echo "[run.sh] Starting QEMU..."
qemu-system-riscv64 \
    -machine virt \
    -cpu rv64 \
    -m 256M \
    -nographic \
    -kernel "$KERNEL_ELF" \
    -initrd "$INITRAMFS" \
    -netdev user,id=net0,hostfwd=tcp::8080-:80 \
    -device virtio-net-device,netdev=net0 \
    -device virtio-blk-device,drive=hd0 \
    -drive file=/dev/null,if=none,id=hd0,format=raw \
    -append "console=ttyS0"
