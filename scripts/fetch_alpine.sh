#!/bin/bash
# Download the unmodified Alpine Linux riscv64 packages (nginx and its
# dependencies, busybox) and unpack them into rootfs/root.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BASE=https://dl-cdn.alpinelinux.org/alpine/v3.22/main/riscv64
PKGS="nginx-1.28.3-r7 musl-1.2.5-r12 pcre2-10.46-r0 zlib-1.3.2-r0 libcrypto3-3.5.8-r0 libssl3-3.5.8-r0 busybox-1.37.0-r20 busybox-static-1.37.0-r20 alpine-baselayout-data-3.7.0-r0 libgcc-14.2.0-r6"
mkdir -p "$ROOT/rootfs/apks" "$ROOT/rootfs/root"
cd "$ROOT/rootfs/apks"
for p in $PKGS; do
    [ -f "$p.apk" ] || curl -sSfO "$BASE/$p.apk"
done
cd "$ROOT/rootfs/root"
for f in ../apks/*.apk; do tar -xzf "$f" 2>/dev/null || true; done
rm -rf .PKGINFO .SIGN.* .pre-install .post-install .trigger .pre-upgrade .post-upgrade .post-deinstall 2>/dev/null || true
echo "unpacked into $ROOT/rootfs/root"
file "$ROOT/rootfs/root/usr/sbin/nginx"
