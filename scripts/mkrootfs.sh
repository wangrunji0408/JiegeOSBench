#!/bin/bash
# Build rootfs.cpio from the extracted Alpine packages plus our own config.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC=$ROOT/rootfs/root
STAGE=$ROOT/rootfs/stage
OUT=$ROOT/rootfs.cpio

rm -rf "$STAGE"
mkdir -p "$STAGE"
# copy package contents (preserve symlinks)
cp -a "$SRC"/. "$STAGE"/

cd "$STAGE"
mkdir -p bin sbin usr/bin usr/sbin etc dev proc sys tmp run var/log/nginx var/lib/nginx/tmp run/nginx var/www/localhost/htdocs root
# busybox applet symlinks (static busybox as /bin/busybox so the shell works
# even before dynamic linking does)
mv bin/busybox bin/busybox.dyn
cp bin/busybox.static bin/busybox
while read -r app; do
    case "$app" in
        busybox) ;;
        *) ln -sf /bin/busybox "bin/$app" ;;
    esac
done < "$ROOT/rootfs/busybox-applets.txt"
# a few things live in sbin traditionally
for a in ifconfig route ip; do [ -e "bin/$a" ] && ln -sf /bin/busybox "sbin/$a"; done

# overlay our files
cp -a "$ROOT/rootfs/overlay"/. "$STAGE"/
chmod +x init

cd "$STAGE"
find . | LC_ALL=C sort | cpio -o -H newc --quiet > "$OUT"
ls -la "$OUT"
