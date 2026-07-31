#!/bin/sh
# Build the initramfs (cpio newc archive) embedded into the kernel.
# Usage: build-initramfs.sh [hello|nginx]
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IM="$ROOT/initramfs"

MODE="${1:-nginx}"
rm -rf "$IM/root"
mkdir -p "$IM/root"

case "$MODE" in
    hello)
        # tiny musl hello for kernel bring-up tests
        cat > "$IM/hello.c" <<'EOF'
#include <unistd.h>
#include <fcntl.h>
int main(void) {
    const char *msg = "hello from JiegeOS userland!\n";
    write(1, msg, 30);
    return 0;
}
EOF
        zig cc -target riscv64-linux-musl -static -O2 -o "$IM/root/init" "$IM/hello.c"
        ;;
    nginx)
        NGINX="$ROOT/nginx/nginx-1.30.4/objs/nginx"
        if [ ! -f "$NGINX" ]; then
            echo "nginx binary not built yet: $NGINX" >&2
            exit 1
        fi
        mkdir -p "$IM/root/usr/local/nginx/conf"
        mkdir -p "$IM/root/usr/local/nginx/html"
        mkdir -p "$IM/root/usr/local/nginx/logs"
        mkdir -p "$IM/root/usr/local/nginx/temp"
        cp "$NGINX" "$IM/root/init"
        chmod +x "$IM/root/init"
        cat > "$IM/root/usr/local/nginx/conf/nginx.conf" <<'EOF'
worker_processes  1;
pid               /usr/local/nginx/logs/nginx.pid;
error_log         stderr info;
daemon            off;

events {
    worker_connections  128;
}

http {
    access_log    /usr/local/nginx/logs/access.log;
    sendfile      off;
    tcp_nopush    on;
    keepalive_timeout  65;

    server {
        listen       80;
        server_name  localhost;
        location / {
            root   /usr/local/nginx/html;
            index  index.html;
        }
    }
}
EOF
        cat > "$IM/root/usr/local/nginx/html/index.html" <<'EOF'
<!DOCTYPE html>
<html>
<head><title>JiegeOS nginx</title></head>
<body style="font-family:monospace;background:#0d1117;color:#c9d1d9;padding:40px">
<h1 style="color:#58a6ff">Hello from JiegeOS + nginx on RISC-V!</h1>
<p>This page is served by the <b>official nginx/1.30.4</b> binary
   (statically linked with musl), running unmodified on a
   from-scratch Rust kernel for RISC-V under QEMU.</p>
<p>Kernel: JiegeOS &mdash; Sv39 paging, virtio-net, TCP/IP, epoll.</p>
</body>
</html>
EOF
        # /etc files (nginx/musl getpwnam fallback)
        mkdir -p "$IM/root/etc"
        cat > "$IM/root/etc/passwd" <<'EOF'
root:x:0:0:root:/root:/bin/sh
nobody:x:65534:65534:nobody:/nonexistent:/sbin/nologin
EOF
        cat > "$IM/root/etc/group" <<'EOF'
root:x:0:
nobody:x:65534:
EOF
        cat > "$IM/root/etc/hosts" <<'EOF'
127.0.0.1 localhost
EOF
        ;;
esac

# empty log files so open(O_APPEND|O_CREAT) works even before creation
: > "$IM/root/usr/local/nginx/logs/error.log" 2>/dev/null || true

cd "$IM/root"
find . -print | cpio -o -H newc > "$IM/initramfs.cpio" 2>/dev/null
cd "$ROOT"
SIZE=$(stat -f%z "$IM/initramfs.cpio" 2>/dev/null || stat -c%s "$IM/initramfs.cpio" 2>/dev/null)
echo "initramfs.cpio built: $SIZE bytes (mode=$MODE)"
