#!/bin/sh
# Build the official nginx 1.30.4 (unmodified source) as a static musl
# riscv64 binary using the zig cc wrapper in tools/.
#
# Notes:
#  * nginx configure compiles small feature-test programs into objs/autotest
#    and then EXECUTES them; a cross-compiled riscv64 ELF cannot run on the
#    build host, so tools/riscv64-musl-cc replaces those outputs with shell
#    scripts that answer the tests (see the wrapper). No nginx source file
#    is modified.
#  * zig cc enables UBSan by default for riscv64-linux-musl targets; we pass
#    -fno-sanitize=all so the shipped binary has no sanitizer runtime (the
#    kernel cannot service libubsan's /proc/self/exe backtrace reads).
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/nginx/nginx-1.30.4"
CC="$ROOT/tools/riscv64-musl-cc"

if [ ! -x "$CC" ]; then
    echo "missing wrapper: $CC" >&2
    exit 1
fi

if [ ! -d "$SRC" ]; then
    echo "nginx source not found: $SRC (untar nginx/nginx-1.30.4.tar.gz)" >&2
    exit 1
fi

cd "$SRC"
rm -rf objs

export CC="$CC"
./configure \
    --crossbuild=Linux:6.0.0:riscv64 \
    --with-cc="$CC" \
    --with-cc-opt="-static -Wno-sign-compare -Wno-conditional-uninitialized" \
    --with-ld-opt="-static" \
    --without-http_rewrite_module \
    --without-http_gzip_module \
    --prefix=/usr/local/nginx

make -f objs/Makefile -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"

echo
echo "nginx built: $SRC/objs/nginx"
"$CC" --version >/dev/null 2>&1 || true
file objs/nginx 2>/dev/null || true
