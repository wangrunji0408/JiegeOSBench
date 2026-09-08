#!/bin/bash
# Prove that the nginx we run is the vendor binary, unmodified.
#
#   tools/verify_binary.sh
#
# It re-extracts /usr/sbin/nginx straight from the official Ubuntu riscv64 .deb
# in tools/debs/, compares it byte-for-byte with the copy in rootfs/, and prints
# the ELF identity plus the package metadata.
set -eu
cd "$(dirname "$0")/.."

DEB=$(ls tools/debs/nginx_*_riscv64.deb | head -1)
echo "== package: $(basename "$DEB")"
echo "== sha256 (deb payload as shipped)"
shasum -a 256 "$DEB"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
ar p "$DEB" data.tar.xz > "$TMP/data.tar.xz" 2>/dev/null || ar p "$DEB" data.tar.zst > "$TMP/data.tar.zst"
tar -xf "$TMP"/data.tar.* -C "$TMP" ./usr/sbin/nginx 2>/dev/null || tar -xf "$TMP"/data.tar.* -C "$TMP" usr/sbin/nginx
echo "== sha256 (binary inside the .deb)"
shasum -a 256 "$TMP/usr/sbin/nginx"
echo "== sha256 (binary in the guest initramfs source tree)"
shasum -a 256 rootfs/usr/sbin/nginx

if cmp -s "$TMP/usr/sbin/nginx" rootfs/usr/sbin/nginx; then
  echo "OK: the nginx we boot is bit-identical to the official Ubuntu riscv64 package"
else
  echo "MISMATCH: rootfs/usr/sbin/nginx differs from the package payload"
  exit 1
fi

echo
echo "== ELF identity"
riscv64-elf-readelf -h rootfs/usr/sbin/nginx | grep -E "Class|Machine|Type|Entry"
echo "== interpreter"
riscv64-elf-readelf -l rootfs/usr/sbin/nginx | grep -A1 INTERP | tail -1
echo "== dynamic dependencies"
riscv64-elf-readelf -d rootfs/usr/sbin/nginx | grep NEEDED
