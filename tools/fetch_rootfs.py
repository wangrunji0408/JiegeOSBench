#!/usr/bin/env python3
"""Fetch and unpack the unmodified Ubuntu 24.04 (noble) riscv64 packages that make
up the guest root filesystem for the iJiege kernel.

Everything here is upstream/vendor-built: we never rebuild nginx or glibc, we only
download the distribution's official riscv64 binaries and unpack them.
"""
import os
import subprocess
import sys
import urllib.request

MIRROR = "http://ports.ubuntu.com/ubuntu-ports"
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CACHE = os.path.join(HERE, "debs")
ROOTFS = os.path.join(ROOT, "rootfs")

# (path in the archive, filename) - all riscv64/all noble builds, unmodified.
PACKAGES = [
    ("pool/main/n/nginx/nginx_1.24.0-2ubuntu7_riscv64.deb", "nginx_1.24.0-2ubuntu7_riscv64.deb"),
    ("pool/main/n/nginx/nginx-common_1.24.0-2ubuntu7_all.deb", "nginx-common_1.24.0-2ubuntu7_all.deb"),
    ("pool/main/g/glibc/libc6_2.39-0ubuntu8_riscv64.deb", "libc6_2.39-0ubuntu8_riscv64.deb"),
    ("pool/main/libx/libxcrypt/libcrypt1_4.4.36-4build1_riscv64.deb", "libcrypt1_4.4.36-4build1_riscv64.deb"),
    ("pool/main/p/pcre2/libpcre2-8-0_10.42-4ubuntu2_riscv64.deb", "libpcre2-8-0_10.42-4ubuntu2_riscv64.deb"),
    ("pool/main/o/openssl/libssl3t64_3.0.13-0ubuntu3_riscv64.deb", "libssl3t64_3.0.13-0ubuntu3_riscv64.deb"),
    ("pool/main/z/zlib/zlib1g_1.3.dfsg-3.1ubuntu2_riscv64.deb", "zlib1g_1.3.dfsg-3.1ubuntu2_riscv64.deb"),
    ("pool/main/g/gcc-14/libgcc-s1_14-20240412-0ubuntu1_riscv64.deb", "libgcc-s1_14-20240412-0ubuntu1_riscv64.deb"),
    ("pool/main/b/busybox/busybox-static_1.36.1-6ubuntu3_riscv64.deb", "busybox-static_1.36.1-6ubuntu3_riscv64.deb"),
]


def fetch(rel, name):
    os.makedirs(CACHE, exist_ok=True)
    dst = os.path.join(CACHE, name)
    if os.path.exists(dst) and os.path.getsize(dst) > 10000:
        return dst
    url = "%s/%s" % (MIRROR, rel)
    print("fetch", url)
    urllib.request.urlopen(url, timeout=120).read()  # probe
    subprocess.check_call(["curl", "-sS", "-m", "300", "-o", dst, url])
    return dst


def unpack(deb, dest):
    """Extract a .deb's data.tar.* into dest (no dpkg needed)."""
    tmp = subprocess.run(["ar", "t", deb], capture_output=True, text=True, check=True).stdout.split()
    member = [m for m in tmp if m.startswith("data.tar")][0]
    subprocess.run(["ar", "p", deb, member], stdout=open("/tmp/_data.tar", "wb"), check=True)
    subprocess.check_call(["tar", "-xf", "/tmp/_data.tar", "-C", dest, "--no-same-owner"])


def main():
    os.makedirs(ROOTFS, exist_ok=True)
    for rel, name in PACKAGES:
        deb = fetch(rel, name)
        print("unpack", name)
        unpack(deb, ROOTFS)
    print("rootfs at", ROOTFS)


if __name__ == "__main__":
    main()
