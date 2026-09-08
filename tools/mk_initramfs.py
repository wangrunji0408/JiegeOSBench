#!/usr/bin/env python3
"""Build the reference initramfs for the iJiege riscv64 Linux reference boot.

The initramfs is a plain "newc" cpio archive (gzipped) that contains:

  * busybox-static (the real Ubuntu riscv64 build) as /bin/busybox plus applet
    symlinks created at boot by `busybox --install -s /bin`
  * the entire pre-unpacked Ubuntu noble riscv64 rootfs (rootfs/): glibc,
    ld.so, nginx, libssl, libpcre2, zlib, /etc/nginx/*, /usr/share/nginx/html
  * strace and its runtime dependencies, taken unmodified from Ubuntu debs
  * libc-bin so that the *real* /usr/bin/ldd is available inside the guest
  * /init (PID 1) and /ref-run.sh, which drive the whole capture and print the
    artifacts to the serial console between machine-readable markers
  * /etc/nginx/nginx.conf = the verbatim production config (ref/nginx.conf)

Nothing is rebuilt; every binary comes from an upstream Ubuntu .deb.

Usage: tools/mk_initramfs.py [--out FILE] [--staging DIR] [--keep-staging]
"""
import argparse
import gzip
import io
import os
import shutil
import stat
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ROOTFS = os.path.join(ROOT, "rootfs")
DEBS = os.path.join(HERE, "debs")
REF = os.path.join(ROOT, "ref")

# Ubuntu debs layered on top of rootfs/ (unmodified upstream binaries)
EXTRA_DEBS = [
    "strace_*.deb",
    "libunwind8_*.deb",
    "libdw1t64_*.deb",
    "libelf1t64_*.deb",
    "libc-bin_*.deb",
    "bash_*.deb",
    "libtinfo6_*.deb",
    "base-files_*.deb",
    "base-passwd_*.deb",
]

# static device nodes baked into the cpio (devtmpfs also gets mounted at boot)
DEVNODES = [
    ("dev/console", 0o600, 5, 1),
    ("dev/null", 0o666, 1, 3),
    ("dev/zero", 0o666, 1, 5),
    ("dev/tty", 0o666, 5, 0),
    ("dev/ttyS0", 0o600, 4, 64),
    ("dev/urandom", 0o666, 1, 9),
    ("dev/random", 0o666, 1, 8),
]

INIT = r"""#!/bin/busybox sh
# PID 1 for the iJiege reference boot: bring up the pseudo filesystems, then
# hand over to /ref-run.sh which drives the nginx + strace capture.
/bin/busybox --install -s /bin 2>/dev/null
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export HOME=/root
export TMPDIR=/tmp

mount -t proc     proc     /proc
mount -t sysfs    sysfs    /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null
mkdir -p /dev/pts /dev/shm
mount -t devpts devpts /dev/pts 2>/dev/null
mount -t tmpfs  tmpfs  /dev/shm 2>/dev/null
# /dev/std{in,out,err} are normally created by systemd; nginx's
# access_log /dev/stdout needs them.
ln -sf /proc/self/fd/0 /dev/stdin
ln -sf /proc/self/fd/1 /dev/stdout
ln -sf /proc/self/fd/2 /dev/stderr
ln -sf /proc/self/fd   /dev/fd
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /run

mkdir -p /run /var/log/nginx /var/cache/nginx /var/lib/nginx /refout
hostname ref-riscv64 2>/dev/null
ifconfig lo up 2>/dev/null || ip link set lo up 2>/dev/null

echo "@@@REF-BOOT: init reached, starting capture@@@"
exec /ref-run.sh
"""

RUN = r"""#!/bin/busybox sh
# Reference capture: run the unmodified Ubuntu nginx under strace, make one
# HTTP request against it, shut it down, and print every artifact to the serial
# console between @@@REF-BEGIN <name>@@@ / @@@REF-END <name>@@@ markers.
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
cd /

mark() { echo "@@@REF-BEGIN $1@@@"; }
endm() { echo "@@@REF-END $1@@@"; }

echo "@@@REF-BOOT: uname/os-release/nginx -v/ldd@@@"
mark uname.txt;       uname -a;                       endm uname.txt
mark os-release.txt;  cat /etc/os-release;            endm os-release.txt
mark nginx-version.txt; /usr/sbin/nginx -v 2>&1;      endm nginx-version.txt
mark ldd.txt;         ldd /usr/sbin/nginx 2>&1;       endm ldd.txt
mark nginx-conf.txt;  cat /etc/nginx/nginx.conf;      endm nginx-conf.txt

echo "@@@REF-BOOT: strace starting@@@"
strace -f -tt -s 200 -o /strace-nginx-run.txt \
    /usr/sbin/nginx -c /etc/nginx/nginx.conf &
STRACE_PID=$!
sleep 2

# ~8 s of traced runtime with a handful of real HTTP requests
echo "@@@REF-BOOT: http request 1@@@"
wget -q -O - http://127.0.0.1/ 2>&1
sleep 1
echo "@@@REF-BOOT: http request 2@@@"
wget -q -O /dev/null http://127.0.0.1/index.html 2>&1
sleep 1
echo "@@@REF-BOOT: http request 3@@@"
wget -q -O /dev/null http://localhost/ 2>&1
sleep 1
echo "@@@REF-BOOT: http request 4@@@"
wget -q -O /dev/null http://127.0.0.1/ 2>&1
sleep 1
echo "@@@REF-BOOT: http request 5@@@"
wget -q -O /dev/null http://127.0.0.1/nonexistent 2>&1
sleep 1

NGINX_PID=$(cat /run/nginx.pid 2>/dev/null)
echo "@@@REF-BOOT: nginx master pid=$NGINX_PID, sending QUIT@@@"
[ -n "$NGINX_PID" ] && kill -QUIT "$NGINX_PID" 2>&1
sleep 2
wait $STRACE_PID 2>/dev/null
echo "@@@REF-BOOT: strace finished@@@"

echo "@@@REF-BOOT: strace line count@@@"
wc -l /strace-nginx-run.txt
echo "@@@REF-BOOT: nginx pid file / leftover processes@@@"
ls -la /run/ 2>&1
ps 2>&1

echo "@@@REF-BOOT: syscall histogram@@@"
# strace -f prefixes every line with the pid; the token after the timestamp
# (or after "pid timestamp" for the main tracee) is the syscall name.
awk '{ if ($1 ~ /^[0-9]+$/) t=$3; else t=$2;
       sub(/\(.*/, "", t);
       if (t ~ /^[a-z_][a-z_0-9]*$/) print t }' /strace-nginx-run.txt \
  | sort | uniq -c | sort -rn > /syscalls.txt
wc -l /syscalls.txt

mark syscalls.txt;            cat /syscalls.txt;            endm syscalls.txt
mark strace-nginx-run.txt;    cat /strace-nginx-run.txt;    endm strace-nginx-run.txt

echo "@@@REF-BOOT: capture complete, powering off@@@"
sync
poweroff -f
sleep 5
"""


def extract_deb(deb, dest):
    """Extract a .deb data.tar.* and merge it into dest.

    Extracting straight into dest fails for packages such as base-files that
    ship /bin, /sbin, /lib as usrmerge symlinks, so we unpack to a scratch tree
    and merge it ourselves (top-level usrmerge links are skipped).
    """
    tmp = "/tmp/_ref_data.tar"
    scratch = "/tmp/_ref_scratch"
    if os.path.exists(scratch):
        shutil.rmtree(scratch)
    os.makedirs(scratch)
    members = subprocess.run(["ar", "t", deb], capture_output=True, text=True,
                             check=True).stdout.split()
    member = [m for m in members if m.startswith("data.tar")][0]
    with open(tmp, "wb") as fh:
        subprocess.run(["ar", "p", deb, member], stdout=fh, check=True)
    subprocess.check_call(["tar", "-xmf", tmp, "-C", scratch,
                           "--no-same-owner"])
    os.unlink(tmp)
    merge_tree(scratch, dest, skip_top={"bin", "sbin", "lib", "lib64"})
    shutil.rmtree(scratch)


def merge_tree(src, dst, skip_top=()):
    """Copy src/* into dst, replacing existing files, skipping skip_top."""
    for root, dirs, files in os.walk(src):
        rel = os.path.relpath(root, src)
        if rel == ".":
            rel = ""
        top = rel.split(os.sep)[0]
        if top in skip_top:
            dirs[:] = []
            continue
        for d in list(dirs):
            if not rel and d in skip_top:
                dirs.remove(d)
                continue
            s = os.path.join(root, d)
            t = os.path.join(dst, rel, d)
            if os.path.islink(s):
                dirs.remove(d)
                if os.path.lexists(t):
                    if os.path.isdir(t) and not os.path.islink(t):
                        shutil.rmtree(t)
                    else:
                        os.unlink(t)
                os.makedirs(os.path.dirname(t), exist_ok=True)
                os.symlink(os.readlink(s), t)
            elif not os.path.isdir(t):
                if os.path.lexists(t):
                    os.unlink(t)
                os.makedirs(t, exist_ok=True)
        for f in files:
            s = os.path.join(root, f)
            t = os.path.join(dst, rel, f)
            os.makedirs(os.path.dirname(t), exist_ok=True)
            if os.path.lexists(t) and (os.path.islink(t) or os.path.isdir(t)):
                if os.path.isdir(t) and not os.path.islink(t):
                    shutil.rmtree(t)
                else:
                    os.unlink(t)
            if os.path.islink(s):
                if os.path.lexists(t):
                    os.unlink(t)
                os.symlink(os.readlink(s), t)
            else:
                shutil.copyfile(s, t)
                os.chmod(t, os.stat(s).st_mode & 0o7777)


def make_staging(staging):
    if os.path.exists(staging):
        shutil.rmtree(staging)
    os.makedirs(staging)

    # 1. pre-unpacked rootfs (glibc + nginx + busybox + /etc/nginx + html)
    p1 = subprocess.Popen(["tar", "-cpf", "-", "-C", ROOTFS, "."],
                          stdout=subprocess.PIPE)
    p2 = subprocess.Popen(["tar", "-xpf", "-", "-C", staging], stdin=p1.stdout)
    p1.stdout.close()
    p2.wait()
    if p1.wait() != 0 or p2.returncode != 0:
        sys.exit("failed to copy rootfs into staging")

    # 2. extra upstream debs (strace, libunwind, libdw, libelf, libc-bin)
    import glob
    for pat in EXTRA_DEBS:
        matches = sorted(glob.glob(os.path.join(DEBS, pat)))
        if not matches:
            sys.exit("missing deb for %s (run tools/ref_fetch_pkgs.py)" % pat)
        deb = matches[-1]
        print("  unpack", os.path.basename(deb))
        extract_deb(deb, staging)

    # 3. merged-/usr symlinks (Ubuntu's usrmerge layout).  Some debs still ship
    # files under /bin or /sbin; merge those into usr/ first.
    for link, target in [("bin", "usr/bin"), ("sbin", "usr/sbin"),
                         ("lib", "usr/lib"), ("lib64", "usr/lib64")]:
        p = os.path.join(staging, link)
        t = os.path.join(staging, target)
        if os.path.islink(p):
            os.unlink(p)
        elif os.path.isdir(p):
            os.makedirs(t, exist_ok=True)
            shutil.copytree(p, t, symlinks=True, dirs_exist_ok=True)
            shutil.rmtree(p)
        os.symlink(target, p)

    # 4. directories the capture needs
    for d in ["dev", "proc", "sys", "tmp", "run", "root", "refout",
              "var/log/nginx", "var/cache/nginx", "var/lib/nginx"]:
        os.makedirs(os.path.join(staging, d), exist_ok=True)

    # 5. busybox must be reachable as /bin/busybox for the /init shebang.
    # /bin is a symlink to usr/bin, so /bin/busybox already resolves to
    # /usr/bin/busybox; applet symlinks are created at boot by --install.
    if not os.path.exists(os.path.join(staging, "bin/busybox")):
        sys.exit("busybox missing from rootfs (expected usr/bin/busybox)")

    # 6. identity/nss files.  base-passwd ships the real /etc/passwd and
    # /etc/group; the www-data user is normally created by nginx-common's
    # postinst (maintainer scripts do not run here), so add it if missing.
    def ensure(rel, content):
        p = os.path.join(staging, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if os.path.exists(p):
            return False
        with open(p, "w") as fh:
            fh.write(content)
        return True

    # base-passwd ships master copies only; materialise them as /etc/passwd
    # and /etc/group the way its postinst would.
    for master, dest in [("usr/share/base-passwd/passwd.master", "etc/passwd"),
                         ("usr/share/base-passwd/group.master", "etc/group")]:
        m = os.path.join(staging, master)
        d = os.path.join(staging, dest)
        if os.path.exists(m) and not os.path.exists(d):
            shutil.copyfile(m, d)

    ensure("etc/passwd",
           "root:x:0:0:root:/root:/bin/sh\n"
           "daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n"
           "nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n")
    ensure("etc/group", "root:x:0:\n" "daemon:x:1:\n" "nogroup:x:65534:\n")
    for rel, line in [("etc/passwd",
                       "www-data:x:33:33:www-data:/var/www:/usr/sbin/nologin\n"),
                      ("etc/group", "www-data:x:33:\n")]:
        p = os.path.join(staging, rel)
        body = open(p).read()
        key = line.split(":")[0]
        if not any(l.startswith(key + ":") for l in body.splitlines()):
            with open(p, "a") as fh:
                fh.write(line)
    ensure("etc/nsswitch.conf",
           "passwd:         files\n"
           "group:          files\n"
           "shadow:         files\n"
           "hosts:          files dns\n"
           "networks:       files\n")
    ensure("etc/hosts", "127.0.0.1\tlocalhost\n127.0.1.1\tref-riscv64\n")
    ensure("etc/hostname", "ref-riscv64\n")
    ensure("etc/resolv.conf", "nameserver 127.0.0.1\n")
    ensure("etc/ld.so.conf", "include /etc/ld.so.conf.d/*.conf\n")

    # 7. verbatim production nginx config
    cfg_src = os.path.join(REF, "nginx.conf")
    if not os.path.exists(cfg_src):
        sys.exit("ref/nginx.conf missing")
    dst = os.path.join(staging, "etc/nginx/nginx.conf")
    shutil.copyfile(cfg_src, dst)

    # 8. init + capture scripts
    for rel, content in [("init", INIT), ("ref-run.sh", RUN)]:
        p = os.path.join(staging, rel)
        with open(p, "w") as fh:
            fh.write(content)
        os.chmod(p, 0o755)

    # 9. static device nodes
    for rel, mode, maj, min_ in DEVNODES:
        p = os.path.join(staging, rel)
        with open(p, "wb"):
            pass
        os.chmod(p, mode)

    return staging


def walk_entries(staging):
    """Yield (relpath, stat) for every entry, directories first."""
    dirs, files = [], []
    for dirpath, dirnames, filenames in os.walk(staging):
        dirnames.sort()
        filenames.sort()
        for d in dirnames:
            full = os.path.join(dirpath, d)
            dirs.append((os.path.relpath(full, staging), os.lstat(full)))
        for f in filenames:
            full = os.path.join(dirpath, f)
            files.append((os.path.relpath(full, staging), os.lstat(full)))
    dirs.sort(key=lambda x: x[0].count("/"))
    return dirs + files


def cpio_newc(staging, out):
    """Write a "newc" cpio archive; returns number of entries."""
    devnode_map = {rel: (maj, min_) for rel, _m, maj, min_ in DEVNODES}
    ino = 1
    entries = 0
    with open(out, "wb") as fh:
        def pad(n):
            return b"" if n % 4 == 0 else b"\0" * (4 - n % 4)

        def header(name, mode, nlink, size, rdev=(0, 0)):
            hdr = "070701"
            hdr += "%08X" % ino
            hdr += "%08X" % (mode & 0xFFFFFFFF)
            hdr += "%08X" % 0      # uid
            hdr += "%08X" % 0      # gid
            hdr += "%08X" % nlink
            hdr += "%08X" % 0      # mtime (deterministic)
            hdr += "%08X" % size
            hdr += "%08X" % 0      # devmajor
            hdr += "%08X" % 0      # devminor
            hdr += "%08X" % rdev[0]
            hdr += "%08X" % rdev[1]
            hdr += "%08X" % (len(name) + 1)
            hdr += "%08X" % 0      # check
            fh.write(hdr.encode("ascii"))
            fh.write(name.encode("utf-8") + b"\0")
            fh.write(pad(len(hdr) + len(name) + 1))

        for rel, st in walk_entries(staging):
            name = rel
            full = os.path.join(staging, rel)
            ino += 1
            if stat.S_ISDIR(st.st_mode):
                header(name, stat.S_IFDIR | (st.st_mode & 0o7777), 2, 0)
            elif stat.S_ISLNK(st.st_mode):
                target = os.readlink(full).encode("utf-8")
                header(name, stat.S_IFLNK | 0o777, 1, len(target))
                fh.write(target)
                fh.write(pad(len(target)))
            elif stat.S_ISCHR(st.st_mode) or name in devnode_map:
                maj, min_ = devnode_map.get(name, (0, 0))
                header(name, stat.S_IFCHR | (st.st_mode & 0o7777), 1, 0,
                       (maj, min_))
            elif stat.S_ISREG(st.st_mode):
                data = open(full, "rb").read()
                header(name, stat.S_IFREG | (st.st_mode & 0o7777), 1, len(data))
                fh.write(data)
                fh.write(pad(len(data)))
            else:
                print("  skipping special file", name)
                continue
            entries += 1

        # TRAILER
        hdr = "070701" + "%08X" % 0 + "%08X" % 0 + "%08X" % 0 + "%08X" % 0
        hdr += "%08X" % 1 + "%08X" % 0 + "%08X" % 0 + "%08X" % 0
        hdr += "%08X" % 0 + "%08X" % 0 + "%08X" % 0 + "%08X" % 11 + "%08X" % 0
        fh.write(hdr.encode("ascii"))
        fh.write(b"TRAILER!!!\0")
        fh.write(pad(len(hdr) + 11))
    return entries


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, "ref", ".build",
                                                  "initramfs.cpio.gz"))
    ap.add_argument("--staging", default=os.path.join(ROOT, "ref", ".build",
                                                      "initramfs"))
    ap.add_argument("--keep-staging", action="store_true")
    args = ap.parse_args()

    print("staging ->", args.staging)
    make_staging(args.staging)

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    raw = args.out + ".raw"
    n = cpio_newc(args.staging, raw)
    with open(raw, "rb") as fin, gzip.open(args.out, "wb", 6) as fout:
        shutil.copyfileobj(fin, fout, 1 << 20)
    os.unlink(raw)
    print("wrote %s (%d entries, %.1f MiB)" %
          (args.out, n, os.path.getsize(args.out) / 1048576.0))
    if not args.keep_staging:
        shutil.rmtree(args.staging)
    print("OK")


if __name__ == "__main__":
    main()
