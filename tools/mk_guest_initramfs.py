#!/usr/bin/env python3
"""Build the guest initramfs (newc cpio, gzip) from rootfs/ plus the extra
configuration files the unmodified Ubuntu nginx needs.

Nothing here rebuilds or patches any vendor binary: files are copied verbatim
from the unpacked Ubuntu riscv64 packages (see fetch_rootfs.py).
"""
import gzip
import os
import stat

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ROOTFS = os.path.join(ROOT, "rootfs")
OUT = os.path.join(ROOT, "build", "initramfs.cpio.gz")

NGINX_CONF = b"""worker_processes 1;
daemon off;
master_process on;
error_log /dev/stderr warn;
pid /run/nginx.pid;
events { worker_connections 256; }
http {
    include /etc/nginx/mime.types;
    default_type application/octet-stream;
    access_log /dev/stdout;
    server {
        listen 80;
        server_name localhost;
        root /usr/share/nginx/html;
        index index.html;
    }
}
"""

EXTRA_FILES = {
    "etc/nginx/nginx.conf": NGINX_CONF,
    "etc/passwd": b"root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n",
    "etc/group": b"root:x:0:\nnogroup:x:65534:\n",
    "etc/nsswitch.conf": b"passwd:         files\ngroup:          files\nshadow:         files\nhosts:          files dns\nnetworks:       files\n",
    "etc/hostname": b"ijiege\n",
    "etc/hosts": b"127.0.0.1 localhost\n10.0.2.15 ijiege\n",
    "sys/devices/system/cpu/online": b"0\n",
    "sys/devices/system/cpu/possible": b"0\n",
    "sys/devices/system/cpu/present": b"0\n",
    "proc/sys/kernel/ngroups_max": b"65536\n",
    "proc/sys/kernel/pid_max": b"4194304\n",
    "proc/sys/crypto/fips_enabled": b"0\n",
}

SYMLINKS = [
    ("lib", "usr/lib"),
    ("lib64", "usr/lib"),
    ("bin", "usr/bin"),
    ("sbin", "usr/sbin"),
    ("var/run", "/run"),
]


def newc_header(name, ino, mode, nlink, filesize, rdev=0):
    def h(v):
        return ("%08x" % v).encode()

    namesize = len(name) + 1
    return (
        b"070701"
        + h(ino)
        + h(mode)
        + h(0)
        + h(0)
        + h(nlink)
        + h(0)
        + h(filesize)
        + h(0)
        + h(0)
        + h(rdev >> 8)
        + h(rdev & 0xFF)
        + h(namesize)
        + h(0)
        + name.encode()
        + b"\0"
    )


def add_file(out, name, ino, mode, data, nlink=1, rdev=0):
    out.write(newc_header(name, ino, mode, nlink, len(data), rdev))
    r = (110 + len(name) + 1) % 4
    if r:
        out.write(b"\0" * (4 - r))
    out.write(data)
    r = len(data) % 4
    if r:
        out.write(b"\0" * (4 - r))


def add_dir(out, name, ino):
    add_file(out, name, ino, 0o040755, b"")


def add_symlink(out, name, ino, target):
    add_file(out, name, ino, 0o120777, target.encode())


def main():
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    ino = 100
    seen = set()
    with gzip.open(OUT, "wb", compresslevel=6) as out:
        add_dir(out, ".", ino)
        ino += 1

        dirs = set()
        for dirpath, dirnames, _ in os.walk(ROOTFS):
            rel = os.path.relpath(dirpath, ROOTFS)
            if rel == ".":
                continue
            for d in dirnames:
                dirs.add(os.path.normpath(os.path.join(rel, d)).replace(os.sep, "/"))
        for d in sorted(dirs):
            if d in seen:
                continue
            seen.add(d)
            add_dir(out, d, ino)
            ino += 1

        for dirpath, dirnames, filenames in os.walk(ROOTFS):
            rel = os.path.relpath(dirpath, ROOTFS)
            for f in filenames:
                p = os.path.join(dirpath, f)
                relp = os.path.normpath(os.path.join(rel, f)).replace(os.sep, "/")
                if relp.startswith("dev/") or relp == "dev":
                    continue
                st = os.lstat(p)
                if stat.S_ISLNK(st.st_mode):
                    if relp in seen:
                        continue
                    add_symlink(out, relp, ino, os.readlink(p))
                elif stat.S_ISREG(st.st_mode):
                    if relp in seen:
                        continue
                    with open(p, "rb") as fh:
                        data = fh.read()
                    add_file(out, relp, ino, 0o100000 | (st.st_mode & 0o7777), data)
                else:
                    continue
                seen.add(relp)
                ino += 1

        for name, target in SYMLINKS:
            if name in seen:
                continue
            seen.add(name)
            add_symlink(out, name, ino, target)
            ino += 1

        for name, data in EXTRA_FILES.items():
            if name in seen:
                continue
            parent = os.path.dirname(name)
            if parent:
                acc = ""
                for p in parent.split("/"):
                    acc = p if not acc else acc + "/" + p
                    if acc not in seen:
                        seen.add(acc)
                        add_dir(out, acc, ino)
                        ino += 1
            seen.add(name)
            add_file(out, name, ino, 0o100644, data)
            ino += 1

        add_file(out, "TRAILER!!!", 0, 0, b"")
    print("wrote %s (%.1f MiB)" % (OUT, os.path.getsize(OUT) / 1048576))


if __name__ == "__main__":
    main()
