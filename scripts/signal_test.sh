#!/bin/bash
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
URL=http://127.0.0.1:18080
C() { curl -sS --noproxy "*" -m 10 "$@"; }
{
  sleep 6
  printf 'cat /run/nginx.pid; nginx -s reload; sleep 2; cat /var/log/nginx/error.log\n'
  sleep 6
  printf 'echo MARK1\n'
  sleep 2
  printf 'nginx -s stop; sleep 2; echo MARK2; cat /var/log/nginx/error.log | tail -8\n'
  sleep 5
  printf 'nginx; sleep 2; echo MARK3\n'
  sleep 6
} | timeout 45 ./run.sh > $LOG 2>&1 &
QPID=$!
sleep 4; echo "=== before reload ==="; C -o /dev/null -w "%{http_code}\n" $URL/
sleep 8; echo "=== after reload ==="; C -o /dev/null -w "%{http_code}\n" $URL/
sleep 8; echo "=== after stop (expect failure) ==="; C -o /dev/null -w "%{http_code}\n" $URL/ 2>&1 | head -2
sleep 8; echo "=== after restart ==="; C -o /dev/null -w "%{http_code}\n" $URL/
wait $QPID
echo "=== guest log ==="
sed -n '/starting init/,$p' $LOG | grep -v "^  0x"
