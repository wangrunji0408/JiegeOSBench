#!/bin/bash
# Boot a real upstream Ubuntu noble riscv64 Linux kernel in QEMU full-system
# emulation with our initramfs, run the unmodified Ubuntu nginx under strace,
# and capture the results from the serial console into ref/.
#
# Stages (all reproducible, rerun from scratch at any time):
#   1. tools/ref_fetch_pkgs.py       -> tools/debs/  (upstream .debs)
#   2. tools/ref_extract_vmlinuz.py  -> ref/.build/vmlinuz-*
#   3. tools/mk_initramfs.py         -> ref/.build/initramfs.cpio.gz
#   4. qemu-system-riscv64 -M virt   -> ref/boot-log.txt
#   5. tools/ref_split_serial.py     -> ref/*.txt
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
REF="$ROOT/ref"
BUILD="$REF/.build"
QEMU="${QEMU:-/opt/homebrew/bin/qemu-system-riscv64}"
TIMEOUT="${TIMEOUT:-1200}"
MEM="${MEM:-2G}"
SMP="${SMP:-1}"
APPEND="${APPEND:-console=ttyS0 rdinit=/init}"

mkdir -p "$REF" "$BUILD"

echo "== [1/5] fetching upstream Ubuntu riscv64 packages"
python3 "$HERE/ref_fetch_pkgs.py" | tail -12

echo "== [2/5] extracting vmlinuz"
KERNEL="$(python3 "$HERE/ref_extract_vmlinuz.py" | tail -1)"
echo "   kernel image: $KERNEL"

echo "== [3/5] building initramfs"
python3 "$HERE/mk_initramfs.py" --out "$BUILD/initramfs.cpio.gz"

echo "== [4/5] booting QEMU (timeout ${TIMEOUT}s)"
QEMU_CMD=("$QEMU" -M virt -m "$MEM" -smp "$SMP"
          -display none -monitor none -no-reboot
          -serial "file:$REF/boot-log.txt"
          -kernel "$KERNEL"
          -initrd "$BUILD/initramfs.cpio.gz"
          -append "$APPEND")
{
    echo "# exact QEMU full-system command line used by tools/ref_boot_linux.sh"
    echo "# kernel: $(basename "$KERNEL")"
    echo "# qemu:   $("$QEMU" --version | head -1)"
    printf '%q ' "${QEMU_CMD[@]}"
    echo
} > "$REF/qemu-command.txt"
cat "$REF/qemu-command.txt"
set +e
timeout "$TIMEOUT" "${QEMU_CMD[@]}"
QEMU_RC=$?
set -e
echo "   qemu exit code: $QEMU_RC (124 = watchdog timeout)"

echo "== [5/5] splitting serial capture into artifacts"
set +e
python3 "$HERE/ref_split_serial.py" "$REF/boot-log.txt"
SPLIT_RC=$?
set -e

echo
echo "serial log: $REF/boot-log.txt ($(wc -l < "$REF/boot-log.txt") lines)"
exit $SPLIT_RC
