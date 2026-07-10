#!/bin/sh
set -eu

BASE=https://dl-cdn.alpinelinux.org/alpine/v3.22/main/riscv64
CACHE=.cache
ROOT=rootfs
mkdir -p "$CACHE" "$ROOT"

for spec in \
  musl-1.2.5-r12 \
  nginx-1.28.3-r4 \
  libcrypto3-3.5.7-r0 \
  libssl3-3.5.7-r0 \
  pcre2-10.46-r0 \
  zlib-1.3.2-r0
do
  archive="$CACHE/$spec.apk"
  if [ ! -f "$archive" ]; then
    curl -fL --retry 3 -o "$archive" "$BASE/$spec.apk"
  fi
  tar -xzf "$archive" -C "$ROOT" 2>/dev/null || true
done

NGINX_APK_SHA256=9a66d023a0654306eb848264b35b8537135a826677ef492fa7476c92e70069c3
actual_apk_sha256=$(shasum -a 256 "$CACHE/nginx-1.28.3-r4.apk" | awk '{print $1}')
[ "$actual_apk_sha256" = "$NGINX_APK_SHA256" ] || {
  echo "nginx APK checksum mismatch" >&2
  exit 1
}

mkdir -p "$ROOT/etc/nginx" "$ROOT/var/lib/nginx/html" "$ROOT/tmp"
cp assets/nginx.conf "$ROOT/etc/nginx/nginx.conf"
cp assets/index.html "$ROOT/var/lib/nginx/html/index.html"

NGINX_BINARY_SHA256=40cf404d4aa6a275c8fc43cd571202323cae0f71717902ae1370241f2148ffc9
actual_binary_sha256=$(shasum -a 256 "$ROOT/usr/sbin/nginx" | awk '{print $1}')
[ "$actual_binary_sha256" = "$NGINX_BINARY_SHA256" ] || {
  echo "nginx binary checksum mismatch" >&2
  exit 1
}
