#!/bin/bash
# Usage: qemu_test.sh TIMEOUT "cmd1" "cmd2" ...  — boots, feeds commands to the shell, prints output.
cd "$(dirname "$0")/.."
T=$1; shift
{
  sleep 2
  for c in "$@"; do
    printf '%s\n' "$c"
    sleep 1.5
  done
  sleep 2
} | timeout "$T" ./run.sh 2>&1 | sed -n '/jiege-os/,$p'
