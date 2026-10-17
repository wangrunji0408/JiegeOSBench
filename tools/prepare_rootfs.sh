#!/bin/sh
# Rebuild the root filesystem from the upstream Alpine riscv64 packages.
#
# The nginx executable placed in the image is byte-for-byte the upstream
# Alpine linux package (which is built from the official nginx 1.26.3
# release sources); it is never patched or recompiled here.
set -eu
cd "$(dirname "$0")/.."

MIRROR=https://dl-cdn.alpinelinux.org/alpine/v3.20
mkdir -p assets
cd assets

fetch() {
    [ -f "$2" ] || curl -sSLO "$1/$2"
}

fetch "$MIRROR/releases/riscv64" alpine-minirootfs-3.20.10-riscv64.tar.gz
fetch "$MIRROR/main/riscv64" nginx-1.26.3-r0.apk
fetch "$MIRROR/main/riscv64" pcre-8.45-r3.apk
fetch "$MIRROR/main/riscv64" zlib-1.3.2-r0.apk

rm -rf alroot apkroot
mkdir -p alroot apkroot
tar -xzf alpine-minirootfs-3.20.10-riscv64.tar.gz -C alroot
for p in nginx-1.26.3-r0 pcre-8.45-r3 zlib-1.3.2-r0; do
    tar -xzf "$p.apk" -C apkroot
done

cd ..
rm -rf rootfs
mkdir -p rootfs/bin rootfs/sbin rootfs/lib rootfs/etc/nginx \
         rootfs/www rootfs/tmp rootfs/dev rootfs/proc \
         rootfs/var/log/nginx rootfs/var/lib/nginx/logs \
         rootfs/var/lib/nginx/tmp rootfs/run

cp assets/alroot/bin/busybox rootfs/bin/
cp assets/alroot/lib/ld-musl-riscv64.so.1 rootfs/lib/
cp assets/alroot/lib/libcrypto.so.3 assets/alroot/lib/libssl.so.3 rootfs/lib/
cp assets/apkroot/usr/lib/libpcre.so.1.2.13 rootfs/lib/
cp assets/apkroot/lib/libz.so.1.3.2 rootfs/lib/
cp assets/apkroot/usr/sbin/nginx rootfs/sbin/nginx
cp assets/apkroot/etc/nginx/mime.types rootfs/etc/nginx/

ln -sf ld-musl-riscv64.so.1 rootfs/lib/libc.musl-riscv64.so.1
ln -sf libpcre.so.1.2.13 rootfs/lib/libpcre.so.1
ln -sf libz.so.1.3.2 rootfs/lib/libz.so.1

cat > rootfs/etc/nginx/nginx.conf <<'EOF'
worker_processes 1;
daemon off;
master_process off;
error_log stderr info;
pid /tmp/nginx.pid;

events {
    worker_connections 128;
}

http {
    include /etc/nginx/mime.types;
    default_type application/octet-stream;
    access_log off;
    sendfile off;
    tcp_nopush off;
    server {
        listen 80;
        server_name localhost;
        root /www;
        index index.html;
        location / {
        }
    }
}
EOF

cat > rootfs/etc/passwd <<'EOF'
root:x:0:0:root:/root:/bin/sh
nginx:x:100:101:nginx:/var/lib/nginx:/sbin/nologin
nobody:x:65534:65534:nobody:/:/sbin/nologin
EOF
cat > rootfs/etc/group <<'EOF'
root:x:0:
nginx:x:101:
nobody:x:65534:
EOF
cat > rootfs/etc/nsswitch.conf <<'EOF'
passwd: files
group: files
hosts: files
EOF

cat > rootfs/www/index.html <<'EOF'
<!DOCTYPE html>
<html><head><title>jiege-os</title></head>
<body><h1>It works!</h1>
<p>This page is served by the unmodified nginx binary running on jiege-os, a RISC-V kernel written in Rust.</p>
</body></html>
EOF

echo "rootfs prepared"
sha256sum rootfs/sbin/nginx 2>/dev/null || shasum -a 256 rootfs/sbin/nginx
