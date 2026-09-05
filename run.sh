#!/bin/bash
# Run the kernel in QEMU. Host port 8080 -> guest port 80.
cd "$(dirname "$0")"
PROFILE=${PROFILE:-release}
KERNEL=kernel/target/riscv64gc-unknown-none-elf/$PROFILE/kernel
HOST_PORT=${HOST_PORT:-18080}
# Wait (briefly) for a previous instance to release the forwarded port.
for _ in $(seq 20); do
    lsof -nP -iTCP:$HOST_PORT -sTCP:LISTEN >/dev/null 2>&1 || break
    sleep 0.5
done
exec qemu-system-riscv64 \
    -machine virt -cpu rv64 -smp 1 -m 512M \
    -nographic \
    -bios default \
    -kernel "$KERNEL" \
    -device loader,file=rootfs.cpio,addr=0x88000000 \
    -netdev user,id=n0,hostfwd=tcp:127.0.0.1:$HOST_PORT-:80 \
    -device virtio-net-device,netdev=n0 \
    ${NETDUMP:+-object filter-dump,id=f0,netdev=n0,file=$NETDUMP} \
    "$@"
