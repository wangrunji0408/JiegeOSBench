#!/bin/bash
# Build rootfs.tar (ustar format, symlinks dereferenced) for embedding into the kernel.
set -e
cd "$(dirname "$0")"
rm -rf build/rootfs
mkdir -p build/rootfs
# copy with symlinks dereferenced
rsync -aL rootfs/ build/rootfs/
# ensure empty runtime dirs survive
mkdir -p build/rootfs/var/log/nginx build/rootfs/tmp build/rootfs/run/nginx \
         build/rootfs/var/lib/nginx/tmp/client_body build/rootfs/dev
cd build/rootfs
tar --format=ustar -cf ../rootfs.tar .
cd ..
ls -la rootfs.tar
