#!/bin/bash
# End-to-end test: boot the iJiege kernel with the unmodified Ubuntu nginx and
# fetch pages from the host through QEMU user-mode networking (hostfwd 8080->80).
#
#   tools/test_http.sh
set -u
cd "$(dirname "$0")/.."
LOG=${LOG:-/tmp/ijiege-http.log}
PORT=${PORT:-8080}

if [ ! -f build/initramfs.cpio ]; then
  echo "building initramfs..."
  python3 tools/mk_guest_initramfs.py
fi

echo "== booting kernel (log: $LOG)"
NET=1 INITRD=build/initramfs.cpio tools/run_qemu.sh > "$LOG" 2>&1 &
QEMU_PID=$!
trap 'kill $QEMU_PID 2>/dev/null' EXIT

# wait for nginx to start listening
for i in $(seq 1 60); do
  if grep -q "launching /usr/sbin/nginx" "$LOG" 2>/dev/null; then break; fi
  sleep 0.5
done
sleep 3

fail=0
check() {
  local desc="$1"; shift
  if "$@" >/tmp/ijiege-check.out 2>&1; then
    echo "PASS  $desc"
  else
    echo "FAIL  $desc"
    sed -n '1,10p' /tmp/ijiege-check.out
    fail=1
  fi
}

echo "== HTTP checks"
check "GET / returns 200 + nginx welcome page" \
  bash -c "curl -sS -m 30 --noproxy '*' -D /tmp/h.txt http://127.0.0.1:$PORT/ -o /tmp/body.html && grep -q '200 OK' /tmp/h.txt && grep -q 'Welcome to nginx' /tmp/body.html"
check "server header is nginx/1.24.0 (Ubuntu)" \
  bash -c "grep -qi 'Server: nginx/1.24.0 (Ubuntu)' /tmp/h.txt"
check "GET /index.html returns 615 bytes" \
  bash -c "curl -sS -m 30 --noproxy '*' http://127.0.0.1:$PORT/index.html -o /tmp/i.html && [ \$(wc -c < /tmp/i.html) -eq 615 ]"
check "GET /nonexistent returns 404" \
  bash -c "curl -sS -m 30 --noproxy '*' -o /dev/null -w '%{http_code}' http://127.0.0.1:$PORT/nonexistent | grep -q 404"
check "10 sequential keep-alive requests" \
  bash -c "for i in \$(seq 1 10); do curl -sS -m 30 --noproxy '*' -o /dev/null -f http://127.0.0.1:$PORT/ || exit 1; done"
check "large file (1 MiB) transfer" \
  bash -c "curl -sS -m 60 --noproxy '*' -o /tmp/big.bin http://127.0.0.1:$PORT/big.bin && [ \$(wc -c < /tmp/big.bin) -eq 1048576 ] && cmp -s /tmp/big.bin rootfs/usr/share/nginx/html/big.bin"
check "5 parallel requests" \
  bash -c "for i in 1 2 3 4 5; do curl -sS -m 30 --noproxy '*' -o /dev/null -f http://127.0.0.1:$PORT/ & done; wait"

echo "== nginx access log (last 12 lines)"
grep -E '"(GET|HEAD)' "$LOG" | tail -12

kill $QEMU_PID 2>/dev/null
wait $QEMU_PID 2>/dev/null
if [ "$fail" = "0" ]; then echo "ALL TESTS PASSED"; else echo "SOME TESTS FAILED"; fi
exit $fail
