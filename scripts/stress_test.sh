#!/bin/bash
# Boot with nginx auto-started; run a battery of HTTP tests from the host.
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
C() { curl -sS --noproxy "*" -m 30 "$@"; }
{ sleep ${HOLD:-60}; } | timeout ${T:-65} ./run.sh > $LOG 2>&1 &
QPID=$!
sleep ${WAIT:-7}
echo "=== index ==="; C -o /dev/null -w "%{http_code} %{size_download}\n" $URL/
echo "=== 404 ==="; C -o /dev/null -w "%{http_code}\n" $URL/nope
echo "=== HEAD ==="; C -I $URL/ | head -1
echo "=== big file (sendfile) ==="; C -o /tmp/big.txt -w "%{http_code} %{size_download} %{speed_download} B/s\n" $URL/big.txt; cmp /tmp/big.txt rootfs/overlay/var/www/localhost/htdocs/big.txt && echo "big.txt identical"
echo "=== keep-alive: 20 requests on one connection ==="
args=(); for i in $(seq 20); do args+=(-o /dev/null -w "%{http_code} " "$URL/?k=$i"); done; C "${args[@]}"; echo
echo "=== 30 parallel connections x 4 requests ==="
start=$(date +%s.%N)
for i in $(seq 30); do ( C -o /dev/null -o /dev/null -o /dev/null -o /dev/null -w "%{http_code}\n" $URL/ $URL/big.txt $URL/nope $URL/index.html 2>&1 | grep -E "^[0-9]+$|curl:" ) & done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
echo "=== POST body ==="; C -o /dev/null -w "%{http_code}\n" -X POST -d "hello=world" $URL/
echo "=== index again ==="; C -o /dev/null -w "%{http_code} %{size_download}\n" $URL/
kill $QPID 2>/dev/null; wait $QPID 2>/dev/null
echo "=== nginx error log (non-info) ==="
sed -n '/jiege-os/,$p' $LOG | grep -E "\[(warn|error|crit|alert|emerg)\]|kernel\] pid" | grep -v io_setup | head
