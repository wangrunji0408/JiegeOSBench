#!/bin/bash
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
N=${N:-100}
C() { curl -sS --noproxy "*" -m 60 "$@"; }
{
  sleep 6
  sleep ${PHASE:-60}
  printf 'echo AFTER; cat /dev/strace; tail -5 /var/log/nginx/error.log\n'
  sleep 3
} | PROFILE=${PROFILE:-release} timeout 80 ./run.sh > $LOG 2>&1 &
QPID=$!
sleep 8
echo "=== $N parallel x 5 requests (big file) ==="
start=$(date +%s.%N)
for i in $(seq $N); do ( C -o /dev/null -o /dev/null -o /dev/null -o /dev/null -o /dev/null -w "%{http_code}\n" $URL/big.txt $URL/ $URL/big.txt $URL/ $URL/nope 2>&1 | grep -E "^[0-9]+$|timed out|reset by peer|Empty reply" ) & done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
wait $QPID
echo "=== guest log ==="
sed -n '/AFTER/,$p' $LOG | grep -v "^  0x\|exited with"
