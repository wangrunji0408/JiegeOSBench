#!/bin/sh
# Boot jiege-os in QEMU and expose the guest HTTP server on 0.0.0.0:8080.
#
#   ./run.sh
#   curl http://127.0.0.1:8080/          (from this host)
#   curl http://<this-host-lan-ip>:8080/ (from another machine)
#
# Quit QEMU with Ctrl-A then X.
set -eu
cd "$(dirname "$0")"

./build.sh

exec qemu-system-riscv64 \
    -machine virt \
    -m 2G \
    -smp 1 \
    -nographic \
    -bios default \
    -kernel target/riscv64gc-unknown-none-elf/release/jiege-os \
    -netdev user,id=net0,hostfwd=tcp:0.0.0.0:8080-:80 \
    -device virtio-net-device,netdev=net0
