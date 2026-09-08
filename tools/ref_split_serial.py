#!/usr/bin/env python3
"""Split the QEMU serial log into the individual reference artifacts.

/ref-run.sh inside the guest prints every artifact between

    @@@REF-BEGIN <name>@@@
    ...content...
    @@@REF-END <name>@@@

so the serial capture (ref/boot-log.txt) is the single source of truth.  This
script carves the artifacts back out of it byte-for-byte (minus the marker
lines themselves) and recomputes ref/syscalls.txt on the host so the histogram
never depends on the guest's busybox awk/grep.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
REF = os.path.join(ROOT, "ref")

# marker name -> artifact path (relative to ref/)
TARGETS = {
    "uname.txt": "uname.txt",
    "os-release.txt": "os-release.txt",
    "nginx-version.txt": "nginx-version.txt",
    "ldd.txt": "ldd.txt",
    "nginx-conf.txt": "nginx-conf.txt",
    "strace-nginx-run.txt": "strace-nginx-run.txt",
}

# strace -f -tt line shapes:
#   "96    16:06:35.123456 execve(\"/usr/sbin/nginx\", ...)"   (pid prefix)
#   "[pid    97] 16:06:35.234567 openat(AT_FDCWD, ...)"        (stderr style)
LINE_PID = re.compile(r"^\d+\s+\d+:\d\d:\d\d\.\d+\s+([a-zA-Z_][a-zA-Z_0-9]*)")
LINE_BRACKET = re.compile(
    r"^\[pid\s+\d+\]\s+\d+:\d\d:\d\d\.\d+\s+([a-zA-Z_][a-zA-Z_0-9]*)")


def syscall_histogram(path):
    counts = {}
    total = 0
    with open(path, "r", errors="replace") as fh:
        for line in fh:
            m = LINE_PID.match(line) or LINE_BRACKET.match(line)
            if not m:
                continue
            name = m.group(1)
            counts[name] = counts.get(name, 0) + 1
            total += 1
    return counts, total


def main():
    log = sys.argv[1] if len(sys.argv) > 1 else os.path.join(REF, "boot-log.txt")
    with open(log, "r", errors="replace") as fh:
        lines = fh.read().splitlines(keepends=True)

    out = {}
    cur = None
    for line in lines:
        s = line.rstrip("\r\n")
        if s.startswith("@@@REF-BEGIN ") and s.endswith("@@@"):
            cur = s[len("@@@REF-BEGIN "):-3]
            out[cur] = []
            continue
        if s.startswith("@@@REF-END ") and s.endswith("@@@"):
            cur = None
            continue
        if cur is not None:
            out[cur].append(line)

    rc = 0
    for name, dest in TARGETS.items():
        body = out.get(name)
        if body is None:
            print("MISSING marker block:", name)
            rc = 1
            continue
        path = os.path.join(REF, dest)
        with open(path, "w") as fh:
            fh.write("".join(body))
        print("%-24s %8d bytes  %6d lines" %
              (dest, os.path.getsize(path), len(body)))

    strace_path = os.path.join(REF, "strace-nginx-run.txt")
    if os.path.exists(strace_path) and os.path.getsize(strace_path) > 0:
        counts, total = syscall_histogram(strace_path)
        path = os.path.join(REF, "syscalls.txt")
        with open(path, "w") as fh:
            for name, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
                fh.write("%7d %s\n" % (n, name))
        print("%-24s %8d bytes  %6d syscall lines, %d distinct" %
              ("syscalls.txt", os.path.getsize(path), total, len(counts)))
    else:
        print("MISSING/empty ref/strace-nginx-run.txt")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
