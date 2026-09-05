# jiege-os — 从零用 Rust 编写的 RISC-V 内核，跑起官方 nginx

一个用 Rust 从头编写的 riscv64 操作系统内核，提供与 Linux 兼容的系统调用接口，
能在 QEMU `virt` 虚拟机上直接运行 **未经修改的 Alpine Linux 官方 nginx 二进制**
（`nginx-1.28.3-r7.apk`，动态链接 musl / OpenSSL / PCRE2 / zlib），并通过 QEMU
用户态网络把宿主机端口转发到虚拟机里的 80 端口，宿主机 `curl` 即可访问网页。

```
┌──────────────── host (macOS) ────────────────┐
│ curl http://127.0.0.1:18080/                 │
│        │  QEMU slirp hostfwd 18080 -> :80    │
│ ┌──────▼─────── QEMU riscv64 virt ─────────┐ │
│ │  nginx master ── fork ──► worker × 2     │ │  ← 官方 Alpine 二进制 + musl ld.so
│ │  busybox sh (init / 调试用)              │ │
│ ├──────────── Linux syscall ABI ───────────┤ │
│ │  jiege-os kernel (Rust, no_std)          │ │
│ │  进程/信号 · Sv39 分页/CoW · ramfs ·     │ │
│ │  epoll · AF_UNIX/SCM_RIGHTS · TCP(smoltcp)│ │
│ │  virtio-net · PLIC · SBI timer · UART    │ │
│ └──────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

## 快速开始

依赖：Rust stable（带 `riscv64gc-unknown-none-elf` target）、`qemu-system-riscv64`、
`curl`、`cpio`。

```bash
./scripts/fetch_alpine.sh         # 下载并解包 Alpine riscv64 软件包（nginx、musl、openssl…）
./scripts/mkrootfs.sh             # 生成 rootfs.cpio（含 nginx 配置、网页、busybox 工具）
(cd kernel && cargo build --release)
PROFILE=release ./run.sh          # 启动 QEMU；init 会自动拉起 nginx，然后进入 busybox shell
```

另开一个终端：

```bash
curl --noproxy '*' http://127.0.0.1:18080/
```

即可看到由 nginx 返回的 `/var/www/localhost/htdocs/index.html`。虚拟机控制台里可以用
`nginx -s reload` / `nginx -s stop` / `nginx` 等命令管理进程；`cat /dev/strace` 查看内核
统计，`echo <pid> > /dev/strace` 打开对某进程的系统调用跟踪。

自动化测试脚本（都在宿主机运行，会自己启动/关闭 QEMU）：

| 脚本 | 内容 |
| --- | --- |
| `scripts/nginx_test.sh` | 基本访问：首页、第二次请求 |
| `scripts/stress_test.sh` | 404 / HEAD / 4 MB 文件 sendfile 校验 / keep-alive / 并发 / POST |
| `scripts/conc_test.sh`, `scripts/conc2.sh` | 50–100 个并发客户端 |
| `scripts/signal_test.sh` | `nginx -s reload` 平滑重载、`-s stop` 优雅退出、再启动 |
| `scripts/leak_test.sh` | 1000 次请求后检查内核堆/页帧/socket 是否回收 |
| `scripts/perf_test.sh` | 吞吐量 |

## 设计

### 启动与硬件
* OpenSBI 引导，内核加载在 `0x8020_0000`；rootfs（cpio newc 归档）由 QEMU 的
  `-device loader` 放到 `0x8800_0000`，启动时解析进内存文件系统。
* 单核，内核态关中断、不可抢占（空闲循环里 `wfi`），用户态可被 100 Hz 时钟抢占。
  锁因此只是一个带重入检测的标记（`sync::SpinLock`）。
* 驱动：16550 UART（中断接收）、PLIC、SBI timer、goldfish RTC（取墙钟时间）、
  virtio-net（`virtio-drivers` raw API，异步批量发送）。

### 内存
* 内核恒等映射物理内存（1 GiB 大页），每个进程的 Sv39 页表都包含这段映射，因此
  trap 不需要切页表。
* 用户地址空间由 VMA 描述（`mm/addrspace.rs`）：按需调页、文件映射（ELF 段直接
  从 ramfs 读）、`fork` 写时复制（`Arc<Frame>` 引用计数）、`MAP_SHARED|MAP_ANON`
  真共享（nginx 的共享内存区）、`brk`/`mmap`/`munmap`/`mprotect`/`mremap` 支持
  VMA 拆分。
* 所有对用户内存的访问都经页表翻译并按页触发缺页，坏指针只会得到 `EFAULT`。

### 进程与信号
* 每个任务一个内核栈，`__switch` 切换 callee-saved 寄存器（含 `fs0–fs11`）；
  trap 入口保存全部 GPR + FPR。
* `fork`（`clone(SIGCHLD)`）、`execve`（ELF + `PT_INTERP` 动态链接器、shebang、
  auxv/AT_RANDOM 等）、`wait4`/`waitid`、进程组/会话、`setsid` 守护进程化。
* 信号：`rt_sigaction/procmask/suspend/timedwait`、`sigaltstack`、完整的
  `ucontext`/`siginfo` 用户栈帧，`rt_sigreturn` 通过映射到每个进程的小 trampoline
  页返回（riscv 上 musl 不带 SA_RESTORER），支持 `SA_RESTART` 语义与 `setitimer`。

### 文件系统
* ramfs（目录/文件/符号链接/设备节点），`/dev/{null,zero,urandom,console,…}`，
  `/dev/stderr` 等 fd 别名，管道、eventfd、epoll（含边沿触发、fd 关闭自动清理）、
  `sendfile`、`getdents64`、`statx`、`ppoll/pselect6` 等。
* AF_UNIX `socketpair`（nginx master/worker 通道）支持 `sendmsg/recvmsg` 传递
  `SCM_RIGHTS` 文件描述符，以及 `EPOLLRDHUP`。

### 网络
* `smoltcp` 提供 IPv4/TCP/UDP，静态地址 `10.0.2.15/24`，网关 `10.0.2.2`。
* 监听 socket 用一组 smoltcp 监听 socket 充当 backlog，`accept` 取出已建立的连接
  并补充；发送路径只做出站处理，让繁忙的 `sendfile` 像 Linux 一样在缓冲区满时返回
  `EAGAIN`，保证多连接公平。
* 单流吞吐约 40 MB/s（release 构建），100 个并发客户端各下载 8 MB 全部成功。

## 目录

```
kernel/          内核源码（Rust, no_std）
  src/trap/      trap 入口汇编、上下文切换、中断分发
  src/mm/        堆、页帧、Sv39 页表、地址空间、用户内存访问
  src/task/      任务、调度、等待队列、信号、ELF 加载与 exec、fork/exit/wait
  src/fs/        ramfs、文件描述符、设备、管道、epoll、eventfd、cpio 加载
  src/net/       smoltcp 接口、TCP/UDP socket、AF_UNIX socketpair
  src/syscall/   Linux 系统调用实现（约 150 个）
  src/drivers/   PLIC、virtio-net
rootfs/          Alpine 软件包（apks/）、解包结果（root/）、覆盖文件（overlay/：
                 init 脚本、nginx.conf、网页、/etc/passwd 里的 nginx 用户）
scripts/         构建 rootfs 与各类自动化测试
run.sh           启动 QEMU（宿主机 127.0.0.1:18080 → 虚拟机 :80）
```

## 已知限制
* 单核；不支持线程（`clone(CLONE_THREAD)` 返回 `EAGAIN`），nginx 默认配置不需要。
* 没有 Linux AIO（`io_setup` 返回 `ENOSYS`，nginx 启动时会打印一条 `[emerg]` 后照常
  运行；只有开启 `aio on` 才会用到）。
* ramfs 全部驻留内存，无持久化；没有 `/proc`。
* 不支持 IPv6，配置里只监听 IPv4。
