#!/bin/sh
# Build the kernel and the embedded root filesystem.
set -eu
cd "$(dirname "$0")"

python3 tools/mkrootfs.py rootfs rootfs.cpio
cargo build --release --offline

echo
echo "built: target/riscv64gc-unknown-none-elf/release/jiege-os"
ls -l target/riscv64gc-unknown-none-elf/release/jiege-os
