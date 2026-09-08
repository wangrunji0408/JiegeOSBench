#!/bin/bash
# Reference: run the same Ubuntu riscv64 nginx under emulated Linux and capture the
# exact syscall sequence it performs. Ground truth for our kernel's ABI.
set -x
docker run --rm --platform linux/riscv64 -v "$PWD/ref":/ref riscv64/ubuntu:24.04 bash -c '
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq strace nginx procps >/dev/null 2>&1 || apt-get install -y -qq strace nginx
mkdir -p /ref
nginx -v 2>&1 | tee /ref/nginx-version.txt
uname -a > /ref/uname.txt
cat /etc/os-release > /ref/os-release.txt
ldd /usr/sbin/nginx > /ref/ldd.txt 2>&1
mkdir -p /tmp/nlog
sed -e "s#/var/log/nginx#/tmp/nlog#g" /etc/nginx/nginx.conf > /tmp/nginx.conf
echo "== strace nginx -v =="
strace -f -o /ref/strace-nginx-v.txt /usr/sbin/nginx -v 2>&1 | tail -3
echo "== strace nginx startup (daemon off, 2s) =="
timeout 3 strace -f -tt -o /ref/strace-nginx-run.txt /usr/sbin/nginx -c /tmp/nginx.conf -g "daemon off;" 2>&1 | tail -5
wc -l /ref/strace-nginx-*.txt
'
