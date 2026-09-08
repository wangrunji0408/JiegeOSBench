#!/bin/bash
# Boot the iJiege kernel under QEMU (riscv64 virt).
#
#   tools/run_qemu.sh [extra qemu args...]
#
# Environment:
#   MEM=2G        guest RAM
#   INITRD=       optional initramfs (cpio.gz)
#   NET=1         attach virtio-net with user-mode (slirp) networking and
#                 host port forward 8080 -> guest 80
#   SMP=1         hart count
#   GDB=1         open a gdb server on :1234 and freeze at start
#   TIMEOUT=0     kill qemu after N seconds (0 = run forever)
set -u
cd "$(dirname "$0")/.."
KERNEL=${KERNEL:-kernel/target/riscv64gc-unknown-none-elf/debug/ijiege-kernel}
MEM=${MEM:-2G}
SMP=${SMP:-1}
TIMEOUT=${TIMEOUT:-0}

ARGS=(-M virt -m "$MEM" -smp "$SMP" -nographic -bios default)
ARGS+=(-kernel "$KERNEL")
[ -n "${INITRD:-}" ] && ARGS+=(-initrd "$INITRD")
if [ "${NET:-0}" = "1" ]; then
  ARGS+=(-netdev "user,id=n0,hostfwd=tcp::8080-:80,hostfwd=tcp::8081-:8081")
  ARGS+=(-device virtio-net-device,netdev=n0)
fi
[ "${GDB:-0}" = "1" ] && ARGS+=(-S -gdb tcp::1234)
if [ "${TIMEOUT:-0}" != "0" ]; then
  exec timeout "$TIMEOUT" qemu-system-riscv64 "${ARGS[@]}" "$@"
else
  exec qemu-system-riscv64 "${ARGS[@]}" "$@"
fi
