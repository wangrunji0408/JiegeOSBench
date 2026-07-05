---
name: ijiege-os-real-linux-binary
description: 内核已能运行真实 Linux glibc 静态二进制——关键陷阱与工具链
metadata:
  type: project
---

截至 2026-07-05，[[ijiege-os-project]] 内核已能在 QEMU 里运行**真实 Linux glibc 静态二进制**（用 Bootlin riscv64-linux-gnu gcc 12.3 + glibc 编译）。

**已验证可工作:** `write/writev/read/exit/exit_group/brk/mmap/munmap/mprotect/getpid/gettid/getuid/geteuid/getgid/getegid/set_tid_address/set_robust_list/futex(占位)/uname/clock_gettime/nanosleep/getrandom/prlimit64/close/fstat/lseek/ioctl/rt_sigaction/rt_sigprocmask/kill`。静态二进制的 printf(stdio,writev)、malloc(brk)、clock_gettime 全部正常。

**关键修复:**
1. **FPU 必须启用**: glibc 用 `fsd` 等保存 FP 寄存器，sstatus.FS=0 时陷入 illegal instruction。在每个用户 TrapContext.sstatus 设 `FS=Dirty(0x6000)`，启动时也 `csrs sstatus, 0x2000`。注意 `csrsi` 只接 5 位立即数，大值用 `csrs sstatus, {}` 寄存器形式。
2. **SUM 位**: 用户页有 U 位，内核(U-mode syscall)读用户 buffer 需 sstatus.SUM=1。在用户 TrapContext.sstatus 里设。
3. **初始栈必须含 argc/argv/envp/auxv**: glibc 从栈读 argc。auxv 需 AT_PHDR/AT_PHNUM/AT_ENTRY/AT_BASE/AT_PAGESZ/AT_RANDOM/AT_NULL。AT_PHDR = 包含 e_phoff 的 PT_LOAD 的 `p_vaddr + (e_phoff - p_offset)`。写初始栈时进程页表未激活——写**栈顶页物理地址**（内核身份映射直写），别写用户 VA。
4. **fstat 必须返回真实 struct stat**（不是全零）: glibc stdio 用 st_blksize；全零会让 stdio 初始化跳到 NULL。填 st_mode=0x81a4(S_IFREG|0644) @off24, st_nlink=1 @16, st_blksize=4096 @56。RISC-V stat 136 字节。

**工具链(关键):**
- Bootlin 工具链是 **x86-64 Linux host** 二进制，macOS aarch64 无法直跑。用 Docker: `docker run --rm --platform linux/amd64 -v $PWD:/work debian:bookworm-slim /work/tools/riscv64-lp64d--glibc--stable-2024.02-1/bin/riscv64-buildroot-linux-gnu-gcc -static -O2 -o x.elf x.c`。需先 `orb start`。
- 工具链 bin 是 x86-64，readelf/objdump 也要在容器跑；用 homebrew `riscv64-elf-readelf`（裸金属，能读任何 ELF）在 macOS 上看。
- 调试 syscall 序列: 临时在 do_syscall 开头 `println!("[sys{}]", num)`。

**待办(极大):** fork/clone(nginx 多 worker)、epoll、signal、文件系统(initramfs/virtio-blk)、TCP/IP 栈+virtio-net+socket、动态链接(官方 nginx 动态链接 glibc/pcre2，或编译静态 nginx)。
