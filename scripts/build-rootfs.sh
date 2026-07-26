#!/bin/bash
# Build the root filesystem image embedded in the kernel.
#
# Downloads the official Alpine riscv64 packages (nginx, musl, and the shared
# libraries nginx links against), unpacks them, adds a minimal nginx.conf and a
# landing page, then produces build/rootfs.tar.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
PKGS="$ROOT/pkgs"
STAGE="$ROOT/build/rootfs"
OUT="$ROOT/build/rootfs.tar"

MIRROR="https://dl-cdn.alpinelinux.org/alpine/edge/main/riscv64"
PACKAGES=(
    nginx-1.30.4-r2
    musl-1.2.6-r2
    pcre2-10.47-r1
    zlib-1.3.2-r0
    libcrypto3-3.5.7-r0
    libssl3-3.5.7-r0
)

mkdir -p "$PKGS" "$STAGE"

echo "==> fetching packages"
for pkg in "${PACKAGES[@]}"; do
    if [[ ! -f "$PKGS/$pkg.apk" ]]; then
        echo "    $pkg"
        curl -sfL -o "$PKGS/$pkg.apk" "$MIRROR/$pkg.apk"
    fi
done

echo "==> unpacking into $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE"
for pkg in "${PACKAGES[@]}"; do
    # apk files are gzipped tars with metadata entries we don't want.
    tar -xzf "$PKGS/$pkg.apk" -C "$STAGE" 2>/dev/null || true
done
rm -f "$STAGE"/.PKGINFO "$STAGE"/.SIGN* "$STAGE"/.pre-* "$STAGE"/.post-* "$STAGE"/.trigger 2>/dev/null || true

echo "==> trimming what the kernel doesn't need"
# Documentation and static libraries only bloat the kernel image.
rm -rf "$STAGE/usr/share/man" "$STAGE/usr/share/doc" "$STAGE/usr/lib/pkgconfig" \
       "$STAGE/usr/include" "$STAGE/usr/lib"/*.a "$STAGE/etc/logrotate.d" \
       "$STAGE/usr/lib/engines-3" "$STAGE/usr/lib/ossl-modules" 2>/dev/null || true

echo "==> creating directories nginx needs at runtime"
# The nginx package ships /var/lib/nginx/{logs,run,modules} as symlinks into
# /var/log and /run; replace them with real directories so the kernel's ramfs
# doesn't have to resolve them.
rm -f "$STAGE"/var/lib/nginx/logs "$STAGE"/var/lib/nginx/run "$STAGE"/var/lib/nginx/modules
mkdir -p "$STAGE"/{dev,proc,sys,tmp,run,root}
mkdir -p "$STAGE"/var/{log,tmp,run}
mkdir -p "$STAGE"/var/log/nginx
mkdir -p "$STAGE"/var/lib/nginx/{tmp,logs,run,html}
mkdir -p "$STAGE"/var/lib/nginx/tmp/{client_body,proxy,fastcgi,uwsgi,scgi}
mkdir -p "$STAGE"/etc/nginx/{http.d,modules,conf.d}

echo "==> writing configuration"
cat > "$STAGE/etc/nginx/nginx.conf" <<'CONF'
# nginx configuration for the jiege kernel.
#
# Runs a single worker in the foreground. The kernel schedules cooperatively on
# one hart, so one worker avoids the accept-mutex contention that several
# workers would introduce for no throughput gain.

user root;
worker_processes 1;

error_log /dev/console info;
pid /run/nginx.pid;

events {
    worker_connections 128;
    use epoll;
}

http {
    include /etc/nginx/mime.types;
    default_type application/octet-stream;

    access_log /dev/console;

    sendfile on;
    tcp_nopush on;
    keepalive_timeout 65;

    # The kernel's ramfs holds everything, so caching open descriptors buys
    # nothing and costs memory.
    open_file_cache off;

    server {
        listen 80 default_server;
        listen [::]:80 default_server;
        server_name _;

        root /var/www;
        index index.html;

        location / {
            try_files $uri $uri/ =404;
        }

        location /status {
            default_type text/plain;
            return 200 "jiege-kernel: nginx is alive\n";
        }
    }
}
CONF

# Passwd and group files, so nginx can resolve the `user` directive.
cat > "$STAGE/etc/passwd" <<'PASSWD'
root:x:0:0:root:/root:/bin/sh
nobody:x:65534:65534:nobody:/:/sbin/nologin
nginx:x:100:101:nginx:/var/lib/nginx:/sbin/nologin
PASSWD

cat > "$STAGE/etc/group" <<'GROUP'
root:x:0:
nobody:x:65534:
nginx:x:101:
GROUP

cat > "$STAGE/etc/hosts" <<'HOSTS'
127.0.0.1	localhost
10.0.2.15	jiege
HOSTS

cat > "$STAGE/etc/resolv.conf" <<'RESOLV'
nameserver 10.0.2.3
RESOLV

echo "jiege" > "$STAGE/etc/hostname"

echo "==> writing the landing page"
mkdir -p "$STAGE/var/www"
cat > "$STAGE/var/www/index.html" <<'HTML'
<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>智能杰哥 · jiege-kernel</title>
<style>
  :root { color-scheme: dark; }
  body {
    margin: 0; min-height: 100vh;
    display: grid; place-items: center;
    background: #0b0f14;
    color: #e6edf3;
    font: 16px/1.6 ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace;
  }
  main { max-width: 46rem; padding: 2.5rem 1.5rem; }
  h1 { margin: 0 0 .25rem; font-size: 1.75rem; letter-spacing: -.01em; }
  .sub { color: #7d8590; margin: 0 0 2rem; }
  .ok {
    display: inline-block; margin-bottom: 1.5rem;
    padding: .3rem .7rem; border-radius: 999px;
    background: #1a3d2e; color: #3fb950;
    font-size: .8rem; letter-spacing: .04em;
  }
  table { width: 100%; border-collapse: collapse; font-size: .9rem; }
  td { padding: .5rem .75rem; border-top: 1px solid #21262d; }
  td:first-child { color: #7d8590; width: 12rem; }
  footer { margin-top: 2rem; color: #484f58; font-size: .8rem; }
  a { color: #58a6ff; }
</style>
</head>
<body>
<main>
  <div class="ok">● HTTP 200 · 服务正常</div>
  <h1>智能杰哥</h1>
  <p class="sub">A RISC-V kernel written from scratch in Rust, running the official nginx binary.</p>

  <table>
    <tr><td>内核 kernel</td><td>jiege-kernel (rv64gc, Sv39 paging)</td></tr>
    <tr><td>服务器 server</td><td>nginx 1.30.4 · unmodified Alpine riscv64 build</td></tr>
    <tr><td>C 库 libc</td><td>musl 1.2.6 · dynamically linked</td></tr>
    <tr><td>网络 network</td><td>virtio-net + smoltcp TCP/IP</td></tr>
    <tr><td>平台 platform</td><td>QEMU virt machine</td></tr>
  </table>

  <footer>
    This page is served by a real nginx process making real Linux syscalls into a
    kernel that implements ELF dynamic loading, fork/exec, signals, futexes,
    epoll, and BSD sockets. Try <a href="/status">/status</a>.
  </footer>
</main>
</body>
</html>
HTML

# The 404/50x pages nginx falls back to.
cat > "$STAGE/var/www/50x.html" <<'HTML'
<!DOCTYPE html><html><head><meta charset="utf-8"><title>Error</title></head>
<body style="background:#0b0f14;color:#e6edf3;font-family:ui-monospace,monospace;text-align:center;padding-top:4rem">
<h1>Server Error</h1><p>jiege-kernel · nginx</p></body></html>
HTML

echo "==> building $OUT"
mkdir -p "$(dirname "$OUT")"
# `tar` on macOS is bsdtar; ask for the portable ustar-with-GNU-extensions form
# our extractor understands, and drop extended attributes.
( cd "$STAGE" && \
  COPYFILE_DISABLE=1 tar --format=ustar --no-mac-metadata -cf "$OUT" . 2>/dev/null || \
  COPYFILE_DISABLE=1 tar --format=ustar -cf "$OUT" . )

echo "==> done: $(du -h "$OUT" | cut -f1) ($(tar -tf "$OUT" | wc -l | tr -d ' ') entries)"
