# iJiege — a from-scratch RISC-V kernel that runs the unmodified nginx binary

**Goal:** write a RISC-V operating-system kernel in Rust from scratch, boot it in
QEMU, and run the vendor-built `nginx` binary on it so that the site it serves is
reachable from outside the virtual machine.

**Result:** `curl http://127.0.0.1:8080/` (host) returns nginx's welcome page,
served by the **byte-identical, unmodified Ubuntu 24.04 riscv64 `nginx` binary**
running on this kernel:

```
$ tools/test_http.sh
PASS  GET / returns 200 + nginx welcome page
PASS  server header is nginx/1.24.0 (Ubuntu)
PASS  GET /index.html returns 615 bytes
PASS  GET /nonexistent returns 404
PASS  10 sequential keep-alive requests
PASS  large file (1 MiB) transfer
PASS  5 parallel requests
ALL TESTS PASSED
```

```
$ curl -D- http://127.0.0.1:8080/
HTTP/1.1 200 OK
Server: nginx/1.24.0 (Ubuntu)
Content-Type: text/html
Content-Length: 615
...
<!DOCTYPE html>
<html>
<head>
<title>Welcome to nginx!</title>
```

Nothing about nginx is rebuilt, patched, re-linked or replaced: `tools/verify_binary.sh`
re-extracts `/usr/sbin/nginx` from the official `.deb` and shows it is bit-identical
to the binary in the guest image. It is a dynamically linked PIE ELF that needs
glibc 2.39, `ld-linux-riscv64-lp64d.so.1`, OpenSSL, PCRE2 and zlib — so the kernel
implements the Linux RISC-V system-call ABI well enough for glibc's dynamic loader
and nginx to run unmodified.

---

## 1. Quick start

```bash
# 1. fetch the vendor packages (glibc + nginx + libs) and unpack them into rootfs/
python3 tools/fetch_rootfs.py

# 2. build the guest initramfs (rootfs + configs, newc cpio)
python3 tools/mk_guest_initramfs.py

# 3. build the kernel
cd kernel && cargo build && cd ..

# 4. boot with a virtio-net device and host port forwarding
NET=1 INITRD=build/initramfs.cpio tools/run_qemu.sh

# 5. from another terminal
curl http://127.0.0.1:8080/
```

`tools/test_http.sh` does all of that and runs the acceptance checks.

Requirements: Rust (stable, target `riscv64imac-unknown-none-elf`), `qemu-system-riscv64`
(≥ 8, tested on 11.1.1), `riscv64-elf-*` binutils for inspection, python3, curl.
Everything runs on macOS or Linux; no cross-compiler is needed for the guest because
the guest userspace is taken verbatim from the vendor packages.

## 2. What the kernel is

A single-hart, Sv39, `no_std` Rust kernel (~9k lines) targeting `qemu-system-riscv64 -M virt`.
It boots via OpenSBI in S-mode at `0x8020_0000`, takes over paging, and implements
a Linux-compatible user ABI.

| Layer | Implementation |
|---|---|
| Boot | `entry.S` clears `.bss`, uses a 128 KiB linker-reserved boot stack, jumps to `rust_main(hartid, dtb)` |
| Firmware | legacy SBI calls (console, timer, shutdown); FDT parser for RAM size + initrd location |
| Traps | full register trap frame in `trap.S`, per-thread trap frame page, separate kernel/user stacks |
| Memory | bitmap physical-frame allocator (4 KiB frames), 64 MiB kernel heap, Sv39 page tables with 2 MiB kernel mappings and a high-half MMIO window |
| Scheduling | round-robin over threads; kernel is non-preemptive (SIE=0), user threads preempted every 8 ms tick; blocking syscalls yield cooperatively |
| Address spaces | per-process page table sharing the kernel's entries; lazy demand paging with `Area`-based VMA list; `brk`, `mmap`/`munmap`/`mprotect`/`mremap` |
| Processes | `execve` (ELF64, PIE + dynamic interpreter), `clone`/`fork`, `wait4`, `exit_group`, credentials, `prctl` |
| ELF | parses program headers, loads segments eagerly, loads `ld-linux-riscv64-lp64d.so.1`, builds the initial stack with `argv`/`envp`/auxv |
| Signals | Linux-identical `rt_sigframe` layout (siginfo + ucontext + `mcontext_t` gregs/fpregs), `rt_sigaction`, `rt_sigprocmask`, `rt_sigsuspend`, `rt_sigreturn`, `SIGCHLD` on child exit, FP register save/restore |
| Files | initramfs (newc cpio) loaded into an in-memory inode tree with symlinks/hardlinks, writable files/dirs (tmpfs semantics), `/proc` and `/sys` stubs, `/dev` char devices |
| Syscalls | 100+ Linux generic-ABI calls (see below), all through one dispatcher |
| Networking | virtio-net MMIO driver (legacy **and** modern transports), smoltcp 0.11 TCP/IPv4, Linux socket layer, listen-pool backlog |
| Block/other | none needed: the root filesystem is an initramfs, everything else is synthetic |

### 2.1 Boot path

```
OpenSBI (M-mode) -> _start (S-mode, 0x80200000)
   clear .bss, sp = sbss (top of the linker-reserved boot stack)
   -> rust_main
        console::init            16550 UART
        fdt::memory()/initrd()   RAM 0x8000_0000.., -initrd placement
        frame::init              reserve kernel image + initrd
        heap::init               64 MiB
        page_table::init_kernel  identity-map RAM (2 MiB pages) + MMIO high half
        csr::set_satp            switch to the kernel page table
        trap::init / plic::init / time::init
        fs::init                 parse the initramfs into the VFS tree
        net::init                probe virtio-mmio slots 0..8
        task::init / start_init  load /usr/sbin/nginx, execve, first user thread
        task::start              idle loop (wfi)
```

### 2.2 Address-space layout (Sv39)

```
0x0000_0000_0000_0000  user text/rodata/data (PIE base 0x0100_0000)
0x0000_0000_0400_0000  ld-linux-riscv64-lp64d.so.1
0x0000_0000_1000_0000  mmap region (libraries, malloc arenas), grows up
0x0000_0000_7e00_0000  rt_sigreturn trampoline page
0x0000_0000_7eff_0000  user stack (16 MiB, grows down)
0x0000_0000_8000_0000  kernel identity map (kernel text/data, all RAM)
0xffff_ffc0_0000_0000  MMIO alias (UART, PLIC, CLINT, RTC, virtio-mmio)
```

The kernel is mapped in every user page table (no KPTI), so traps never need a
page-table switch; the MMIO alias exists because the user space occupies Sv39
entries 0–1 and the devices live at physical addresses below 0x4000_0000.

### 2.3 Threads, traps and signals

Each thread owns a kernel stack and a dedicated trap-frame page. `sscratch` always
points at the current thread's trap frame, so the trap entry can save the full
register file without any global state. `__restore` writes `sscratch` again before
`sret`, which makes the scheme re-entrant for both user and kernel traps. FP
registers are not touched by the kernel (it is built for the soft-float
`riscv64imac` target), and each thread's FP file is saved/restored across context
switches and signal frames.

Signals reproduce Linux's exact `rt_sigframe`: 128-byte `siginfo_t`, then
`ucontext_t` with `stack_t`, 128-byte `sigset_t` and `mcontext_t`
(`user_regs_struct` + 528-byte FP state), plus the `rt_sigsuspend` mask semantics
(the temporary mask stays in effect until the signal is actually delivered).

## 3. The Linux ABI surface that actually gets used

The reference environment (`ref/`) records a real `strace -f` of the same nginx
binary on a real Linux kernel. The kernel implements every syscall that trace
touches (56 distinct), including the subtle ones:

* **process**: `execve`, `clone` (`CLONE_CHILD_CLEARTID|CLONE_CHILD_SETTID|SIGCHLD`),
  `exit_group`, `wait4(WNOHANG)`, `getpid`/`gettid`/`getppid`, `set_tid_address`,
  `set_robust_list`, `prctl(PR_SET_DUMPABLE)`, `setuid`/`setgid`/`setgroups`
* **memory**: `brk`, `mmap` (`MAP_PRIVATE|MAP_FIXED|MAP_DENYWRITE`,
  `MAP_PRIVATE|MAP_ANONYMOUS`, `MAP_SHARED|MAP_ANONYMOUS`), `mprotect`, `munmap`,
  `mremap`, `madvise`
* **files**: `openat` (`O_RDONLY|O_CLOEXEC`, `O_WRONLY|O_CREAT|O_APPEND`,
  `O_RDWR|O_CREAT|O_TRUNC`), `read`, `pread64`, `write`, `writev`, `pwrite64`,
  `fstat`, `newfstatat`, `faccessat`, `lseek`, `mkdirat`, `unlinkat`, `dup3`,
  `fcntl`, `ioctl(FIONBIO/FIOASYNC/TCGETS)`, `getdents64`, `readlinkat`,
  `sendfile`, `statfs`
* **sockets**: `socket`, `setsockopt(SO_REUSEADDR)`, `bind`, `listen`, `accept4`,
  `recvfrom`, `sendmsg`, `recvmsg`, `socketpair`, `connect` (AF_UNIX path →
  `ENOENT`, which is what glibc's nscd probe expects), `getsockname`, `shutdown`
* **events**: `epoll_create1`, `epoll_ctl`, `epoll_pwait` (with the correct
  riscv64 `struct epoll_event` layout), `eventfd2`, `ppoll`, `pselect6`
* **signals**: `rt_sigaction` (`SA_SIGINFO`), `rt_sigprocmask`, `rt_sigsuspend`,
  `rt_sigreturn`, real `SIGCHLD` with `si_pid`/`si_status`
* **misc**: `futex` (`FUTEX_WAIT`/`WAKE`/private), `getrandom`, `uname`,
  `prlimit64`, `clock_gettime`, `gettimeofday`, `nanosleep`, `sched_getaffinity`,
  `getrusage`, `sysinfo`

The reference trace also pinned down the filesystem surface nginx expects:
`/etc/nginx/{nginx.conf,mime.types}`, `/etc/{passwd,group,nsswitch.conf}`,
`/sys/devices/system/cpu/online`, `/proc/sys/kernel/ngroups_max`,
`/dev/{stdout,stderr,null,urandom}`, `/run/nginx.pid`,
`/var/lib/nginx/{body,proxy,fastcgi,uwsgi,scgi}` and the six shared libraries —
all of which are either provided by the vendor rootfs or synthesised at boot.

## 4. Networking and the "reachable from outside" path

```
host curl -> QEMU user-mode networking (slirp, hostfwd tcp::8080-:80)
          -> virtio-net MMIO (guest 10.0.2.15/24, gw 10.0.2.2)
          -> kernel TCP/IP (smoltcp 0.11) -> Linux socket layer -> nginx accept4()
```

* The virtio-net driver supports both the legacy (MMIO v1, 10-byte header) and
  modern (MMIO v2, 12-byte header) transports, because QEMU 11's `virt` machine
  still exposes `virtio-net-device` as legacy by default. Add
  `-global virtio-mmio.force-legacy=off` to exercise the modern path.
* smoltcp has no listen backlog, so the socket layer keeps a **pool of 16
  listening sockets per port**; `tcp_accept` scans the pool and re-arms the slot it
  took the connection from. Without this, a 20-way parallel `curl` burst lost
  ~40 % of connections to RST; with it, 100/100 succeed.
* Blocking syscalls poll the stack and sleep for one 1 ms tick, which keeps the
  scheduler simple while still moving ~100 requests/s and 1 MiB files fine.
* The kernel pumps the device from the PLIC interrupt handler (with `try_lock`,
  so an interrupt during a syscall just acknowledges) and from the idle loop.

## 5. Repository layout

```
kernel/            the Rust kernel crate
  src/entry.S      boot
  src/trap.S       trap entry/restore, context switch
  src/fp.S         FP register save/restore (raw encodings, soft-float target)
  src/mm/          frames, heap, Sv39 page tables
  src/task/        threads, scheduler, processes, ELF loader, signals
  src/syscall/     Linux syscall dispatcher
  src/fs/          VFS (initramfs + tmpfs), files, pipes, char devices, cpio
  src/net/         virtio-net driver + smoltcp stack + socket API
  src/socket.rs    Linux socket layer (AF_INET, AF_UNIX socketpair, epoll glue)
  linker.ld        kernel memory layout (note the reserved boot stack)
tools/
  fetch_rootfs.py        download + unpack the official Ubuntu riscv64 packages
  mk_guest_initramfs.py  build the guest initramfs (newc cpio) from rootfs/
  run_qemu.sh            boot the kernel (MEM/INITRD/NET/APPEND/TIMEOUT knobs)
  test_http.sh           end-to-end acceptance test
  verify_binary.sh       prove the nginx binary is the vendor binary, unmodified
  ref_*                  the reference-Linux harness (real kernel + strace)
ref/                 ground truth: strace of nginx on a real Linux kernel
rootfs/              unpacked vendor packages (glibc, nginx, libs)
build/               generated initramfs
```

## 6. Reproducing the reference ground truth

`ref/README.md` documents a full-system QEMU boot of an upstream Ubuntu riscv64
Linux kernel with the same nginx binary, captured with `strace -f`. That trace
(`ref/strace-nginx-run.txt`, 306 lines, master + worker) is what the kernel's
syscall layer was written against. `tools/ref_boot_linux.sh` re-runs the whole
pipeline (fetch packages, extract `vmlinuz`, build the initramfs, boot, capture).

## 7. Known limitations

* Single hart; no SMP, no threads (`clone` with `CLONE_VM|CLONE_THREAD` returns
  `ENOSYS` — nginx does not use them, `fork` works).
* No block device or real filesystem: the root filesystem is an initramfs, writes
  live in memory. `mount`/`chroot` are not implemented.
* No copy-on-write; `fork` copies every mapped page.
* smoltcp gives IPv4 TCP only: no UDP/DNS, no fragmentation, no TCP options
  beyond what smoltcp negotiates. `sendfile` is emulated with pread/write.
* `tcp_shutdown(read)` is a no-op (smoltcp 0.11 has no receive-half close).
* No KPTI, no ASLR (fixed PIE base), no vDSO (a small `rt_sigreturn` trampoline
  page stands in for it).
* Soft-float kernel (`riscv64imac`) by design, so user FP state is never touched
  by the kernel; per-thread FP save/restore happens only on context switches and
  signal delivery.
