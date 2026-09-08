# Reference: unmodified Ubuntu riscv64 nginx under full-system QEMU/Linux

This directory is the **ground truth** for the iJiege Linux-ABI-compatible kernel:
it records exactly what the vendor-built Ubuntu 24.04 (noble) riscv64 `nginx`
binary does on a real upstream Linux kernel, booted in QEMU full-system
emulation. Nothing is rebuilt or patched — every binary is an upstream Ubuntu
riscv64 `.deb` payload.

The earlier attempt (`tools/ref_strace.sh`) failed because `ptrace`/`strace` do
not work under user-mode emulation (OrbStack/`docker run --platform linux/riscv64`
returns `ENOSYS` for `PTRACE_TRACEME`). That is why this reference uses
`qemu-system-riscv64` with a real kernel instead.

---

## 1. Environment actually used

| Item | Value |
|---|---|
| Kernel | `7.0.0-31-generic` (`#31.1~24.04.1-Ubuntu SMP PREEMPT_DYNAMIC Tue Aug 25 15:43:59 UTC`, `Ubuntu 7.0.0-31.31.1~24.04.1-generic 7.0.14`) |
| Kernel package | `linux-image-7.0.0-31-generic_7.0.0-31.31.1~24.04.1_riscv64.deb` (noble-updates, `linux-image-generic` meta target) |
| Firmware | OpenSBI `v1.8.1` (bundled with QEMU, `-bios default`) |
| Machine | `-M virt`, `-m 2G`, `-smp 1`, RV64GC+ (`rv64imafdch`, Sstc, Zicbo*, Svadu) |
| QEMU | `QEMU emulator version 11.1.1` (`/opt/homebrew/bin/qemu-system-riscv64`) |
| Userspace | Ubuntu 24.04.5 LTS (noble) riscv64 |
| glibc | `libc6 2.39-0ubuntu8`, `libc-bin 2.39-0ubuntu8.8`, loader `/lib/ld-linux-riscv64-lp64d.so.1` |
| nginx | `nginx/1.24.0 (Ubuntu)` from `nginx_1.24.0-2ubuntu7_riscv64.deb` + `nginx-common` |
| strace | `strace 6.8-0ubuntu2` (riscv64), `-f -tt -s 200` |
| nginx libs | libcrypt.so.1, libpcre2-8.so.0, libssl.so.3, libcrypto.so.3, libz.so.1, libc.so.6 |
| Init | `/init` (busybox sh) on initramfs `newc` cpio, `rdinit=/init`, no root device |
| Networking | loopback only (`ifconfig lo up`); requests from busybox `wget` to `127.0.0.1` |

## 2. Exact command lines

**Stage 1 — fetch upstream packages** (writes `tools/debs/`):

```
python3 tools/ref_fetch_pkgs.py
```

Parses `http://ports.ubuntu.com/ubuntu-ports/dists/{noble,noble-updates}/main/binary-riscv64/Packages.gz`
and downloads: `linux-image-generic` (+ its `linux-image-<ver>-generic` target),
`strace`, `libunwind8`, `libdw1t64`, `libelf1t64`, `libc-bin`, `bash`, `libtinfo6`,
`base-files`, `base-passwd`, `linux-libc-dev`.

**Stage 2 — extract the bootable kernel**:

```
python3 tools/ref_extract_vmlinuz.py
```

`boot/vmlinuz-7.0.0-31-generic` is *both* a PE32+ EFI application and a flat
RISC-V Linux `Image`: the first instruction `c.li s4,-13` decodes to ASCII
`MZ`, `e_lfanew` at `0x3c` points to the PE header at `0x40`, and the RISC-V
image header (`"RISCV"` + magic `0x05435352`) sits at `0x30/0x38` with
`text_offset=0x200000`. QEMU's `-kernel` loader therefore accepts the file
unmodified — no extraction/objcopy needed.

**Stage 3 — build the initramfs** (writes `ref/.build/initramfs.cpio.gz`):

```
python3 tools/mk_initramfs.py
```

Pure-Python `newc` cpio writer (no host `cpio` dependency, gzip'd). Contains the
full `rootfs/` (glibc + nginx + busybox), strace + deps, `libc-bin`'s real
`/usr/bin/ldd` + `bash`, `/etc/nginx/nginx.conf` (verbatim production config),
`/init`, `/ref-run.sh`, static `/dev` nodes, and a merged-`/usr` layout
(`/bin → usr/bin`, `/lib → usr/lib`, …). `/init` mounts proc/sysfs/devtmpfs and
creates `/dev/std{in,out,err} → /proc/self/fd/*` (systemd normally does this;
nginx's `access_log /dev/stdout` needs them).

**Stage 4 — boot and capture** (writes `ref/boot-log.txt`, the full serial log):

```
qemu-system-riscv64 -M virt -m 2G -smp 1 -display none -monitor none -no-reboot \
  -serial file:ref/boot-log.txt \
  -kernel ref/.build/vmlinuz-7.0.0-31-generic \
  -initrd ref/.build/initramfs.cpio.gz \
  -append "console=ttyS0 rdinit=/init"
```

Inside the guest, `/ref-run.sh` (PID 1 after `exec`) runs exactly:

```
strace -f -tt -s 200 -o /strace-nginx-run.txt /usr/sbin/nginx -c /etc/nginx/nginx.conf
```

then makes 5 HTTP requests (`busybox wget` to `127.0.0.1/`, `/index.html`,
`localhost/`, `/`, `/nonexistent`), sends `kill -QUIT <master pid>`, waits for
strace, and `cat`s each artifact to the serial console between
`@@@REF-BEGIN <name>@@@` / `@@@REF-END <name>@@@` markers.

**Stage 5 — carve artifacts out of the serial log**:

```
python3 tools/ref_split_serial.py ref/boot-log.txt
```

**One-shot reproduction** (runs all five stages):

```
./tools/ref_boot_linux.sh
```

Boot to `/init` takes ~1.4 s of guest time; the whole run (boot + ~7.3 s of
traced nginx) finishes in well under a minute on Apple Silicon TCG.

## 3. Artifacts

| File | Contents |
|---|---|
| `boot-log.txt` | complete QEMU serial console log (OpenSBI + kernel boot + guest capture) |
| `strace-nginx-run.txt` | complete `strace -f -tt -s 200` of nginx master+worker, startup through 5 requests and graceful shutdown (306 lines, 2 pids) |
| `syscalls.txt` | sorted syscall-name histogram computed from the trace (56 distinct) |
| `nginx-version.txt` | `nginx -v` |
| `ldd.txt` | `ldd /usr/sbin/nginx` (real glibc `ldd`) |
| `uname.txt` | `uname -a` |
| `os-release.txt` | `/etc/os-release` (bonus) |
| `nginx-conf.txt` | the config as read from `/etc/nginx/nginx.conf` |
| `nginx.conf` | source of that config (verbatim production text) |
| `qemu-command.txt` | exact QEMU command line + kernel + QEMU version |

## 4. What the binaries actually do (ordered)

Trace shape: pid **96** = nginx master (the `execve`'d process), pid **97** = the
single worker (`clone`d, never `exec`'d). `strace -f -tt` prefixes every line
with the pid; the master and worker interleave.

1. **ld.so startup (master)**
   `execve("/usr/sbin/nginx", ["/usr/sbin/nginx","-c","/etc/nginx/nginx.conf"], envp)` →
   `brk(NULL)` → `faccessat(AT_FDCWD,"/etc/ld.so.preload",R_OK)` →
   `openat("/etc/ld.so.cache", O_RDONLY|O_CLOEXEC)` (**ENOENT here — no cache built**,
   so every library is resolved by path) → for each DT_NEEDED library
   (`libcrypt.so.1`, `libpcre2-8.so.0`, `libssl.so.3`, `libcrypto.so.3`,
   `libz.so.1`, `libc.so.6`):
   `openat(...,O_RDONLY|O_CLOEXEC)` → `read(fd, …, 832)` (ELF header) →
   `fstat` → `mmap(NULL, sz, PROT_READ|PROT_EXEC, MAP_PRIVATE|MAP_DENYWRITE, fd, 0)` →
   `mmap(addr, sz2, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_FIXED|MAP_DENYWRITE, fd, off)` →
   optional `mmap(…, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_FIXED|MAP_ANONYMOUS, -1, 0)` (bss) → `close`.
   Then `mmap(NULL, 8192, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)`,
   `set_tid_address(tcb)`, `set_robust_list(tcb+0x10, 24)`, a series of
   `mprotect(addr, len, PROT_READ)` for RELRO segments, `prlimit64(0, RLIMIT_STACK, NULL, …)`,
   `getrandom(buf, 8, GRND_NONBLOCK)` (pointer guard), `brk(0x…)` heap growth.
   **No `rseq`, no `arch_prctl`, no `riscv_hwprobe`, no `sigaltstack`** on this
   glibc 2.39/riscv64 build.
2. **glibc / library init**
   `openat("/etc/localtime", O_RDONLY|O_CLOEXEC)` (ENOENT, falls back to UTC),
   `getpid`, `getppid`, ~16× `futex(FUTEX_WAKE_PRIVATE, INT_MAX)` (OpenSSL/libcrypto
   one-time init), `openat("/proc/sys/crypto/fips_enabled", O_RDONLY)` (ENOENT),
   `openat("/usr/lib/ssl/openssl.cnf", O_RDONLY)` (ENOENT).
3. **nginx configuration**
   `uname()` (twice), `openat("/sys/devices/system/cpu/online", O_RDONLY|O_CLOEXEC)` +
   `read` (glibc `sysconf(_SC_NPROCESSORS_ONLN)`; **nginx does not call
   `sched_getaffinity` on this build**), `prlimit64(RLIMIT_NOFILE)`,
   `openat("/etc/nginx/nginx.conf", O_RDONLY)` → `fstat` → `pread64` (whole 380 B) →
   `epoll_create1(0)` + `close` (probe), `brk`,
   `openat("/etc/nginx/mime.types")` → `fstat` → 2× `pread64` (4096 + 1369 B).
4. **privilege drop preparation (master, uid 0)**
   `geteuid` → `socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK)` +
   `connect("/var/run/nscd/socket")` **ENOENT** (×4, glibc nscd probe before each
   nsswitch lookup) → `newfstatat("/etc/nsswitch.conf")` →
   `openat("/etc/nsswitch.conf")`/`fstat`/`read`/`read(0)`/`close` →
   `openat("/etc/passwd")` + `fstat` + `lseek(0,SEEK_SET)` + `read` (getpwnam("nobody")) →
   same nscd dance + `openat("/etc/group")` + `read` (getgrnam).
5. **nginx temp paths** (created unconditionally by the core module)
   5× `mkdirat("/var/lib/nginx/{body,proxy,fastcgi,uwsgi,scgi}", 0700)` +
   5× `newfstatat` + 5× `fchownat(…, 65534, -1, 0)`.
6. **log fds and listening socket**
   `openat("/dev/stderr", O_WRONLY|O_CREAT|O_APPEND, 0644)` + `fcntl(FD_CLOEXEC)`,
   `openat("/dev/stdout", O_WRONLY|O_CREAT|O_APPEND, 0644)` + `fcntl(FD_CLOEXEC)`,
   `socket(AF_INET, SOCK_STREAM, IPPROTO_IP)` → `setsockopt(SO_REUSEADDR, [1])` →
   `ioctl(FIONBIO, [1])` → `bind(0.0.0.0:80)` → `listen(511)` (called twice) →
   `prlimit64(RLIMIT_NOFILE)` →
   `mmap(NULL, 1280, PROT_READ|PROT_WRITE, MAP_SHARED|MAP_ANONYMOUS, -1, 0)` (shared
   connection counters).
7. **signal setup (master)**
   12× `rt_sigaction(…, sa_flags=SA_SIGINFO)` for HUP, USR1, WINCH, TERM, QUIT,
   USR2, ALRM, INT, IO, CHLD; `SIGSYS`/`SIGPIPE` → `SIG_IGN`.
8. **pid file / daemonisation**
   `openat("/run/nginx.pid", O_RDWR|O_CREAT|O_TRUNC, 0644)` → `pwrite64("96\n")` → `close`;
   `dup3(stderr_fd, 2, 0)`; `rt_sigprocmask(SIG_BLOCK, {HUP,INT,QUIT,USR1,USR2,ALRM,TERM,CHLD,WINCH,IO})`.
9. **master↔worker channel and fork**
   `socketpair(AF_UNIX, SOCK_STREAM, 0, [6,7])` → `ioctl(FIONBIO)` ×2 →
   `ioctl(FIOASYNC, [1])` → `fcntl(F_SIGOWN)` → `fcntl(F_SETFD, FD_CLOEXEC)` ×2 →
   `clone(child_stack=NULL, flags=CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD, child_tidptr=…)`.
   The master then parks in `rt_sigsuspend([], 8)`.
10. **worker init (pid 97, still uid 0)**
    `set_robust_list`, `getpid`, `geteuid`, `setgid(65534)` →
    `openat("/proc/sys/kernel/ngroups_max")` + `read` (glibc `initgroups`) →
    `newfstatat("/etc/nsswitch.conf")` ×2 → `openat("/etc/group")`/`read` →
    `setgroups(1, [65534])` → `setuid(65534)` → `prctl(PR_SET_DUMPABLE, SUID_DUMP_USER)` →
    `rt_sigprocmask(SIG_SETMASK, [])` → `epoll_create1(0)` → `eventfd2(0,0)` →
    `epoll_ctl(ADD, EPOLLIN|EPOLLET)` → `socketpair(AF_UNIX)` +
    `epoll_ctl(ADD, EPOLLIN|EPOLLRDHUP|EPOLLET)` → `close` →
    `epoll_pwait(…, 5000, …)` (channel handshake) → `brk` →
    `epoll_ctl(ADD, listen_fd, EPOLLIN|EPOLLRDHUP)` → `close(master end of channel)` →
    `epoll_ctl(ADD, channel_fd, EPOLLIN|EPOLLRDHUP)` →
    `epoll_pwait(…, 512, -1, NULL, 8)` — the worker event loop.
11. **one HTTP request (worker)**
    `accept4(listen_fd, …, SOCK_NONBLOCK)` → `epoll_ctl(ADD, conn, EPOLLIN|EPOLLRDHUP|EPOLLET)` →
    `epoll_pwait(…, 60000)` → `recvfrom(conn, buf, 1024, 0, NULL, NULL)` (request line + headers) →
    `newfstatat("/usr/share/nginx/html/index.html")` →
    `openat(…, O_RDONLY|O_NONBLOCK)` → `fstat` → `pread64(fd, buf, 615, 0)` →
    `writev(conn, [status+headers, body], 2)` →
    `write(access_log_fd=1, "127.0.0.1 - - […] \"GET / HTTP/1.1\" 200 615 …")` →
    `close(file)`, `close(conn)`, back to `epoll_pwait`.
    For `GET /nonexistent`: `newfstatat` → `openat` fails → `write(error_log_fd=3, "…[error]… open() \"/usr/share/nginx/html/nonexistent\" failed (2: No such file or directory)…")` →
    `writev(conn, [404 headers, body, footer], 3)`.
12. **graceful shutdown (`kill -QUIT`)**
    master `rt_sigsuspend` returns `ERESTARTNOHAND` →
    `sendmsg(channel_fd, 32-byte ngx_channel_t {command=3=quit, pid=master})` →
    worker `recvmsg(channel_fd)` → `gettid` → `recvmsg` again → `-1 EAGAIN` →
    `epoll_ctl(DEL)`, `close(listen_fd)`, `close(channel)`,
    `futex(FUTEX_WAKE_PRIVATE)`, `exit_group(0)` →
    master receives `SIGCHLD {si_code=CLD_EXITED, si_pid=97, si_uid=65534, si_status=0}` →
    `wait4(-1, …, WNOHANG)` ×2 (second returns `-1 ECHILD`) →
    `rt_sigreturn` → `close(channel_fd)` ×2 → `unlinkat("/run/nginx.pid", 0)` →
    `futex` → `exit_group(0)`.

### Filesystem paths touched (complete)

`/usr/sbin/nginx`, `/etc/ld.so.preload`, `/etc/ld.so.cache`, `/etc/localtime`,
`/etc/nsswitch.conf`, `/etc/passwd`, `/etc/group`,
`/proc/sys/crypto/fips_enabled`, `/proc/sys/kernel/ngroups_max`,
`/sys/devices/system/cpu/online`, `/usr/lib/ssl/openssl.cnf`,
`/etc/nginx/nginx.conf`, `/etc/nginx/mime.types`, `/dev/stderr`, `/dev/stdout`,
`/run/nginx.pid`, `/var/run/nscd/socket`, `/var/lib/nginx/{body,proxy,fastcgi,uwsgi,scgi}`,
`/usr/share/nginx/html/{index.html,nonexistent}`, plus the six shared libraries
under `/lib/riscv64-linux-gnu/` and `/lib/ld-linux-riscv64-lp64d.so.1`.

### Notable / surprising details

* **No `rseq`, no `sched_getaffinity`, no `arch_prctl`, no `sigaltstack`, no
  `statx`, no `io_uring`, no `clone3`** in this trace. Process creation is
  classic `clone(CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD)` and file
  metadata uses `fstat`/`newfstatat` (never `statx`).
* `/etc/ld.so.cache` is missing, so every library load is a pathname `openat`;
  a kernel only needs `openat`+`read`+`mmap`+`fstat` to make ld.so happy.
* The 4 `socket(AF_UNIX)`/`connect("/var/run/nscd/socket")` pairs are glibc's
  nscd probe (`ENOENT` is the expected/required answer) — they happen before
  *each* `getpwnam`/`getgrnam`/`initgroups` and must not abort the lookup.
* `fchownat(…, 65534, -1, 0)` uses `uid=65534, gid=-1` (`AT_SYMLINK_NOFOLLOW`
  not set) — `-1` means "leave group unchanged".
* nginx opens `/dev/stdout` and `/dev/stderr` as regular `O_WRONLY|O_CREAT|O_APPEND`
  files and then `dup3`s the error fd onto fd 2. `/dev/stdout` **must exist**
  (symlink to `/proc/self/fd/1`); a bare devtmpfs does not provide it.
* `pwrite64` (not `write`) writes the pid file; `sendmsg`/`recvmsg` carry the
  32-byte `ngx_channel_t` over the `AF_UNIX` socketpair.
* Signal handling: 12 `rt_sigaction` registrations with `SA_SIGINFO`, master
  blocks 10 signals around `fork`, and shutdown is driven by `SIGCHLD` +
  `wait4(WNOHANG)` + `rt_sigreturn`.
* Privilege drop happens **in the worker** (`setgid` → `initgroups` →
  `setgroups` → `setuid` → `prctl(PR_SET_DUMPABLE, SUID_DUMP_USER)`), so the
  kernel must support those in a process that has already been `clone`d.
* Every request logs with a single `write()` to the access-log fd and every
  response uses a single `writev()` (2 iovecs for 200, 3 for 404) — no
  `sendfile` because the config does not enable it.

## 5. Syscall histogram (from `syscalls.txt`, 295 traced lines, 56 distinct)

```
    36 close                     2 getpid
    27 openat                    2 listen
    18 mmap                      2 recvmsg
    17 fstat                     2 rt_sigprocmask
    17 futex                     2 rt_sigreturn
    14 read                      2 rt_sigsuspend
    13 newfstatat                2 set_robust_list
    12 epoll_pwait               2 socketpair
    12 rt_sigaction              2 uname
    10 epoll_ctl                 2 wait4
     8 mprotect                  1 bind
     7 pread64                   1 clone
     6 write                     1 dup3
     5 accept4                   1 eventfd2
     5 brk                       1 execve
     5 fchownat                  1 faccessat
     5 fcntl                     1 getppid
     5 mkdirat                   1 getrandom
     5 recvfrom                  1 gettid
     5 socket                    1 prctl
     5 writev                    1 pwrite64
     4 connect                   1 sendmsg
     4 ioctl                     1 set_tid_address
     3 lseek                     1 setgid
     3 prlimit64                 1 setgroups
     2 epoll_create1             1 setsockopt
     2 exit_group                1 setuid
     2 geteuid                   1 unlinkat
```

Top 40 by count:

```
close 36, openat 27, mmap 18, fstat 17, futex 17, read 14, newfstatat 13,
epoll_pwait 12, rt_sigaction 12, epoll_ctl 10, mprotect 8, pread64 7, write 6,
accept4 5, brk 5, fchownat 5, fcntl 5, mkdirat 5, recvfrom 5, socket 5,
writev 5, connect 4, ioctl 4, lseek 3, prlimit64 3, epoll_create1 2,
exit_group 2, geteuid 2, getpid 2, listen 2, recvmsg 2, rt_sigprocmask 2,
rt_sigreturn 2, rt_sigsuspend 2, set_robust_list 2, socketpair 2, uname 2,
wait4 2, bind 1, clone 1
```

## 6. Minimum ABI surface implied by this trace

For a kernel that must run this exact nginx, the following syscall families are
exercised end-to-end and must be correct (not just present):

* **Process/thread**: `execve`, `clone` (`CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD`),
  `exit_group`, `wait4` (`WNOHANG`), `getpid`, `gettid`, `getppid`, `set_tid_address`,
  `set_robust_list`, `prctl(PR_SET_DUMPABLE)`.
* **Credentials**: `geteuid`, `setgid`, `setgroups`, `setuid`, `fchownat` (uid 65534, gid -1).
* **Memory**: `brk`, `mmap` (`MAP_PRIVATE|MAP_FIXED|MAP_DENYWRITE`, `MAP_PRIVATE|MAP_ANONYMOUS`,
  `MAP_SHARED|MAP_ANONYMOUS`), `mprotect`, and `mmap` of ELF segments at the loader's
  hint addresses.
* **Files/FS**: `openat` (`O_RDONLY|O_CLOEXEC`, `O_RDONLY|O_NONBLOCK`,
  `O_WRONLY|O_CREAT|O_APPEND`, `O_RDWR|O_CREAT|O_TRUNC`), `close`, `read`, `pread64`,
  `write`, `writev`, `pwrite64`, `fstat`, `newfstatat`, `faccessat`, `lseek`,
  `mkdirat` (0700), `unlinkat`, `dup3`, `fcntl` (`F_SETFD`, `F_SETOWN`), `ioctl`
  (`FIONBIO`, `FIOASYNC`).
* **Sockets**: `socket` (AF_INET/AF_UNIX), `setsockopt(SO_REUSEADDR)`, `bind`,
  `listen`, `accept4` (`SOCK_NONBLOCK`), `recvfrom`, `sendmsg`, `recvmsg`,
  `socketpair`, `connect` (expected `ENOENT` on `/var/run/nscd/socket`).
* **Polling/events**: `epoll_create1`, `epoll_ctl` (`ADD`/`DEL`, `EPOLLIN|EPOLLET|EPOLLRDHUP`),
  `epoll_pwait` (timeouts -1/5000/60000), `eventfd2`.
* **Signals**: `rt_sigaction` (`SA_SIGINFO`, `SIG_IGN`), `rt_sigprocmask`,
  `rt_sigsuspend`, `rt_sigreturn`, real `SIGCHLD` delivery with `si_pid`/`si_status`.
* **Sync/misc**: `futex` (`FUTEX_WAKE_PRIVATE`), `getrandom` (`GRND_NONBLOCK`),
  `uname`, `prlimit64` (`RLIMIT_STACK`, `RLIMIT_NOFILE`).
* **Pseudo-FS**: `/proc/sys/kernel/ngroups_max`, `/proc/sys/crypto/fips_enabled`,
  `/sys/devices/system/cpu/online`, `/proc/self/fd/*` (for `/dev/std*`).
  All are read as ordinary files; missing ones must return `ENOENT` rather than
  fail the caller.
