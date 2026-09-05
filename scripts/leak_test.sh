#!/bin/bash
# Many connections, then read kernel stats from inside the guest.
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
C() { curl -sS --noproxy "*" -m 20 "$@"; }
{
  sleep 6
  printf 'cat /dev/strace\n'
  sleep ${PHASE:-25}
  printf 'echo AFTER; cat /dev/strace\n'
  sleep 12
  printf 'echo LATER; cat /dev/strace\n'
  sleep 2
} | PROFILE=${PROFILE:-release} timeout 60 ./run.sh > $LOG 2>&1 &
QPID=$!
sleep 8
echo "=== 1000 sequential requests ==="
start=$(date +%s.%N)
for i in $(seq 1000); do C -o /dev/null -w "%{http_code}\n" $URL/; done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
echo "=== 100 parallel x 5 requests (big file) ==="
start=$(date +%s.%N)
for i in $(seq 100); do ( C -o /dev/null -o /dev/null -o /dev/null -o /dev/null -o /dev/null -w "%{http_code}\n" $URL/big.txt $URL/ $URL/big.txt $URL/ $URL/nope 2>&1 ) & done | sort | uniq -c
end=$(date +%s.%N); echo "elapsed: $(echo "$end - $start" | bc)s"
wait $QPID
echo "=== guest log ==="
sed -n '/starting init/,$p' $LOG | grep -v "^  0x\|exited with"
