#!/bin/bash
# Concurrency test: N parallel curl clients, each fetching several URLs.
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
N=${N:-50}
C() { curl -sS --noproxy "*" -m 20 "$@"; }
{ sleep ${HOLD:-60}; } | timeout ${T:-65} ./run.sh > $LOG 2>&1 &
QPID=$!
sleep ${WAIT:-6}
echo "=== $N parallel clients x 4 requests ==="
start=$(date +%s.%N)
for i in $(seq $N); do ( C -o /dev/null -o /dev/null -o /dev/null -o /dev/null -w "%{http_code}\n" $URL/ $URL/big.txt $URL/nope $URL/index.html 2>&1 ) & done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
echo "=== sequential 200 requests (keep-alive off) ==="
start=$(date +%s.%N)
for i in $(seq 200); do C -o /dev/null -w "%{http_code}\n" -H "Connection: close" $URL/; done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
echo "=== index again ==="; C -o /dev/null -w "%{http_code} %{size_download}\n" $URL/
kill $QPID 2>/dev/null; wait $QPID 2>/dev/null
echo "=== guest log (tail) ==="
sed -n '/jiege-os/,$p' $LOG | grep -v "^  0x" | tail -15
