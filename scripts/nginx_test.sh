#!/bin/bash
# Boot, start nginx, curl from the host, report.
cd "$(dirname "$0")/.."
LOG=/tmp/jiege-nginx.log
rm -f $LOG
{
  sleep ${WAIT:-6}
  printf 'cat /run/nginx.pid\n'
  sleep ${HOLD:-8}
} | timeout ${T:-30} ./run.sh > $LOG 2>&1 &
QPID=$!
sleep $((2 + ${WAIT:-6} + 1))
echo "=== curl from host ==="
curl -sS --noproxy "*" -m 5 -i http://127.0.0.1:18080/ ; echo "curl exit=$?"
echo "=== second request ==="
curl -sS --noproxy "*" -m 5 -o /dev/null -w "%{http_code} %{size_download} bytes\n" http://127.0.0.1:18080/index.html; echo "curl exit=$?"
wait $QPID
echo "=== guest log ==="
sed -n '/jiege-os/,$p' $LOG | grep -v "^  0x"
