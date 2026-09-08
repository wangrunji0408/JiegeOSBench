#!/usr/bin/env python3
"""Fetch the extra upstream Ubuntu 24.04 (noble) riscv64 packages needed for the
full-system reference boot: a real Linux kernel image plus strace and its
runtime dependencies.

Everything is vendor-built upstream; nothing is rebuilt here.  The package
indexes are parsed live so the script keeps working when noble-updates rolls a
new kernel.

Outputs (unmodified .deb files) go to tools/debs/.
"""
import gzip
import io
import os
import subprocess
import sys
import urllib.request

MIRROR = "http://ports.ubuntu.com/ubuntu-ports"
HERE = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(HERE, "debs")

# noble main + noble-updates main (kernel lives in -updates)
SUITES = ["noble", "noble-updates"]
COMPONENT = "main"
ARCH = "riscv64"

# meta package whose target kernel we want, plus explicit extra packages.
WANT = [
    "linux-image-generic",   # -> depends on linux-image-<ver>-generic (the real image)
    "strace",
    "libunwind8",
    "libdw1t64",
    "libelf1t64",
    "libc-bin",            # provides the real /usr/bin/ldd used in ref/ldd.txt
    "bash",                # /usr/bin/ldd is a #!/bin/bash script
    "libtinfo6",
    "base-files",          # authentic /etc/os-release
    "base-passwd",         # authentic /etc/passwd, /etc/group (www-data, nobody)
    "linux-libc-dev",
]


def fetch_index(suite):
    url = "%s/dists/%s/%s/binary-%s/Packages.gz" % (MIRROR, suite, COMPONENT, ARCH)
    print("index", url)
    data = urllib.request.urlopen(url, timeout=300).read()
    return gzip.decompress(data).decode("utf-8", "replace")


def parse(text):
    """Return list of package stanzas (dicts)."""
    out = []
    cur = {}
    for line in text.splitlines():
        if not line.strip():
            if cur.get("Package"):
                out.append(cur)
            cur = {}
            continue
        if line[0] in " \t":
            continue
        k, _, v = line.partition(":")
        cur[k.strip()] = v.strip()
    if cur.get("Package"):
        out.append(cur)
    return out


def version_key(v):
    """Crude debian version ordering good enough to pick the newest build."""
    epoch, _, rest = v.partition(":")
    if not rest:
        epoch, rest = "0", v
    main, _, rev = rest.partition("-")

    def nums(s):
        # tokenise on debian separators, then make every token mutually comparable
        toks = [x for x in __import__("re").split(r"[.~+:]", s) if x != ""]
        return tuple((1, int(x)) if x.isdigit() else (0, x) for x in toks)

    return (int(epoch), nums(main), nums(rev))


def build_db():
    db = {}
    for suite in SUITES:
        for st in parse(fetch_index(suite)):
            key = st["Package"]
            prev = db.get(key)
            if prev is None or version_key(st["Version"]) > version_key(prev["Version"]):
                db[key] = st
    return db


def download(st):
    os.makedirs(CACHE, exist_ok=True)
    name = os.path.basename(st["Filename"])
    dst = os.path.join(CACHE, name)
    if os.path.exists(dst) and os.path.getsize(dst) >= int(st["Size"]):
        print("cached", name)
        return dst
    url = "%s/%s" % (MIRROR, st["Filename"])
    print("fetch", url, st["Size"])
    subprocess.check_call(["curl", "-fSL", "--retry", "3", "-m", "900", "-o", dst, url])
    return dst


def main():
    db = build_db()
    todo = []
    for name in WANT:
        st = db.get(name)
        if st is None:
            sys.exit("package not found in index: %s" % name)
        todo.append(st)
        # follow hard Depends one level for the kernel meta package
        if name == "linux-image-generic":
            for dep in st.get("Depends", "").split(","):
                dep = dep.strip().split(" ")[0].split("(")[0].strip()
                if dep.startswith("linux-image-") and dep in db:
                    todo.append(db[dep])
    seen = set()
    for st in todo:
        if st["Package"] in seen:
            continue
        seen.add(st["Package"])
        download(st)
    print("\npackage -> version")
    for st in todo:
        print("  %-40s %s" % (st["Package"], st["Version"]))


if __name__ == "__main__":
    main()
