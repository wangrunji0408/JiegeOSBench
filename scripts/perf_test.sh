#!/bin/bash
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
C() { curl -sS --noproxy "*" -m 60 "$@"; }
{ sleep ${HOLD:-70}; } | PROFILE=${PROFILE:-release} timeout 75 ./run.sh > $LOG 2>&1 &
QPID=$!
sleep 7
echo "=== single stream 4MB x3 ==="
for i in 1 2 3; do C -o /dev/null -w "%{http_code} %{size_download} bytes %{speed_download} B/s %{time_total}s\n" $URL/big.txt; done
for n in 10 20 40; do
  echo "=== $n parallel x 2 big files ==="
  start=$(date +%s.%N)
  for i in $(seq $n); do ( C -o /dev/null -o /dev/null -w "%{http_code}\n" $URL/big.txt $URL/big.txt 2>&1 | grep -E "^[0-9]+$|timed out|reset|Empty" ) & done | sort | uniq -c
  end=$(date +%s.%N); el=$(echo "$end - $start" | bc); echo "elapsed: ${el}s => $(echo "$n * 8 / $el" | bc) MB/s aggregate"
done
kill $QPID 2>/dev/null; wait $QPID 2>/dev/null
