#!/bin/sh
set -eu

make build
LOG=${TMPDIR:-/tmp}/jiege-qemu.$$.log
BODY=${TMPDIR:-/tmp}/jiege-body.$$.html
HEADERS=${TMPDIR:-/tmp}/jiege-headers.$$.txt

qemu-system-riscv64 \
  -machine virt -global virtio-mmio.force-legacy=false \
  -bios default -kernel target/riscv64gc-unknown-none-elf/release/jiege-kernel \
  -m 256M -smp 1 \
  -netdev user,id=net0,hostfwd=tcp::8080-:80 \
  -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56 \
  -nographic -monitor none -no-reboot >"$LOG" 2>&1 &
QEMU_PID=$!

cleanup() {
  kill "$QEMU_PID" 2>/dev/null || true
  wait "$QEMU_PID" 2>/dev/null || true
  rm -f "$BODY" "$HEADERS"
}
trap cleanup EXIT INT TERM

attempt=0
while ! grep -q 'using the "epoll" event method' "$LOG"; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 30 ]; then
    echo "FAIL: nginx did not enter its event loop" >&2
    tail -80 "$LOG" >&2
    exit 1
  fi
  sleep 1
done

request() {
  curl --noproxy '*' -fsS --max-time 10 -D "$HEADERS" http://127.0.0.1:8080/ -o "$BODY"
  grep -q '^HTTP/1.1 200 OK' "$HEADERS"
  grep -qi '^Server: nginx/1.28.3' "$HEADERS"
  grep -q 'nginx on Jiege OS' "$BODY"
}

request
request
echo "PASS: official nginx served two sequential HTTP connections"
echo "QEMU log: $LOG"
