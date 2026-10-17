# jiege-os

一个从零开始、用 Rust 编写的 **RISC-V 64 位操作系统内核**。它不依赖任何现成的
OS 代码，直接运行在 QEMU `virt` 机器上，并成功运行 **未经修改的 nginx 官方
二进制**（Alpine riscv64 官方包，nginx 1.26.3），通过 QEMU 用户态网络把网页
服务暴露到宿主机（以及局域网）的 `0.0.0.0:8080`。

```
$ curl -i http://127.0.0.1:8080/
HTTP/1.1 200 OK
Server: nginx/1.26.3
Content-Type: text/html
Content-Length: 211
...
<!DOCTYPE html>
<html><head><title>jiege-os</title></head>
<body><h1>It works!</h1>
<p>This page is served by the unmodified nginx binary running on jiege-os,
a RISC-V kernel written in Rust.</p>
</body></html>
```

---

## 1. 快速开始

依赖：`qemu-system-riscv64`、Rust（`riscv64gc-unknown-none-elf` target）、
`python3`。第三方 crate 已 vendor 到 `vendor/`，可离线构建。

```sh
./build.sh     # 生成 rootfs.cpio 并编译内核
./run.sh       # 启动 QEMU，宿主机 8080 -> 客户机 80
```

另开一个终端：

```sh
curl http://127.0.0.1:8080/
# 或从局域网另一台机器：
curl http://<宿主机 IP>:8080/
```

`run.sh` 使用 `hostfwd=tcp:0.0.0.0:8080-:80`，因此监听在所有网卡上；
`lsof -iTCP:8080` 可以看到 `TCP *:8080 (LISTEN)`。

退出 QEMU：`Ctrl-A` 然后 `X`。

---

## 2. 目标与约束

* 目标程序必须是真正的 nginx 可执行文件，不能替换成自己写的 HTTP server，
  也不能打补丁。本项目运行的是 Alpine Linux 官方 riscv64 包中的 nginx：

  * 包：`nginx-1.26.3-r0.apk`（Alpine v3.20，riscv64）
  * SHA-256：`bd4a7b1b448ec07e20089c6ab2761dab115740753f715669ed7f94a367efdbe5`
  * 与 `assets/apkroot/usr/sbin/nginx` 完全一致，构建前后校验和不变。

  我们只提供它运行所需的 Linux ABI、动态链接器、libc 和文件系统；nginx 本体
  一个字节都没有改动。

* 单进程运行（`nginx.conf` 中 `master_process off;`）。这是 nginx 官方支持的
  运行模式，属于配置层面的选择，不修改二进制。因此内核不需要实现 `fork`/
  `clone` 的进程复制。

* 为了让移植聚焦，运行时关闭了 `sendfile`（`sendfile off;`），走
  `read`/`writev` 路径；这也只是配置项。

---

## 3. 总体设计

```
                 QEMU virt (RISC-V64, 2 GiB)
  +-------------------------------------------------------------+
  |  OpenSBI (M-mode)                                           |
  |    | ecall / sret                                          |
  |  jiege-os (S-mode kernel, Rust no_std)                      |
  |    |                                                        |
  |    |  U-mode: nginx + ld-musl + libssl/libcrypto/libpcre/libz|
  |    |  (ELF, musl dynamic linking, ecall Linux syscalls)     |
  |                                                             |
  |  virtio-mmio net  <-->  smoltcp TCP/IP  <-->  socket/epoll  |
  +-------------------------------------------------------------+
                          |
                    QEMU slirp (user networking)
                          |
                 host 0.0.0.0:8080 -> guest 10.0.2.15:80
```

### 3.1 启动与特权级

* `_start`（汇编）建立栈、清零 `.bss`，跳入 `kmain`。
* 内核链接在 `0x8020_0000`，由 OpenSBI 以 S-mode 载入；UART 直接驱动
  `0x1000_0000` 的 16550。
* 用户程序运行在 **U-mode**，系统调用经 `ecall` 陷入 S-mode 的 `trap_entry`。
  内核保存 32 个通用寄存器 + `sepc`/`sstatus`（`TrapFrame`），用独立的
  trap 栈（`sscratch` 交换）。
* 不启用中断（纯轮询），因此没有 PLIC/时钟中断处理；时间来自 `rdtime`
  （QEMU 10 MHz）。

### 3.2 地址空间（无 MMU 的单地址空间）

为了把精力集中在 Linux 兼容层而不是页表/缺页处理上，内核使用 **satp=0 的
单地址空间**（U-mode 直接使用物理地址），布局固定：

| 区域 | 地址 |
| --- | --- |
| 内核镜像 + 内嵌 rootfs | `0x8020_0000` … |
| 内核堆（free-list 分配器） | `0x8400_0000`，32 MiB |
| nginx 主程序（PIE 装载） | `0x8800_0000` |
| 用户栈（8 MiB） | 顶 `0x8F00_0000` |
| mmap 区（ld.so / libc / 堆） | `0x9000_0000` … `0xA800_0000` |
| 动态链接器 `ld-musl` | `0xB000_0000` |

`mmap`/`munmap`/`mremap`/`brk`/`mprotect` 都实现了；`mmap` 支持匿名内存和
文件映射（ld.so 通过 `MAP_FIXED` 把 ELF 段映射到预留区域内）。

### 3.3 ELF 与 musl 动态链接

* 解析 ELF64/RISC-V（`PT_LOAD`、`PT_INTERP`），支持 PIE 的 load bias。
* 内核把 `/sbin/nginx` 装到 `0x8800_0000`，把
  `/lib/ld-musl-riscv64.so.1` 装到 `0xB000_0000`，构造 Linux 初始栈：
  `argc/argv/envp/auxv`，包含 `AT_PHDR/AT_PHENT/AT_PHNUM/AT_BASE/AT_ENTRY/
  AT_RANDOM/AT_HWCAP/AT_EXECFN` 等。
* 之后完全由 musl 的 `ld.so` 自己加载 `libssl.so.3`、`libcrypto.so.3`、
  `libpcre.so.1`、`libz.so.1` 并做重定位。

### 3.4 Linux 系统调用兼容层

按 `asm-generic`（riscv64）编号实现 syscall，返回值采用 `-errno` 约定。
覆盖 nginx 实际用到的全部接口，包括：

* 文件：`openat/close/read/write/writev/pread64/pwrite64/lseek/fstat/
  newfstatat/getdents64/readlinkat/faccessat/mkdirat/unlinkat/chdir/getcwd`
* 内存：`brk/mmap/munmap/mremap/mprotect/madvise`
* 进程：`exit_group/set_tid_address/set_robust_list/rseq/prctl/getpid/getuid/
  uname/sysinfo/getrlimit/prlimit64/getrusage`
* 时间：`clock_gettime/gettimeofday/nanosleep/clock_nanosleep`
* 信号（记录接口，无实际投递）：`rt_sigaction/rt_sigprocmask/sigaltstack`
* I/O 复用：`epoll_create1/epoll_ctl/epoll_pwait`（注意 riscv64 上
  `struct epoll_event` 是 16 字节，和 x86_64 的 packed 12 字节不同）
* 网络：`socket/bind/listen/accept4/connect/getsockname/getpeername/
  setsockopt/getsockopt/sendto/recvfrom/shutdown`
* 其它：`ioctl(FIONBIO/FIONREAD)`、`fcntl`、`dup/dup3`、`pipe2`、`eventfd2`、
  `socketpair`、`getrandom`、`riscv_hwprobe`、`riscv_flush_icache`、
  `io_setup`（nginx 初始化 AIO 上下文用）

### 3.5 文件系统

* 内核用 `include_bytes!` 内嵌一个 newc cpio 根文件系统（约 6.8 MB），启动时
  解析成内存中的节点表：只读的 cpio 文件 + 可写的 RAM overlay。
* 支持目录、符号链接、`stat`、`getdents64`；`/dev/null`、`/dev/zero`、
  `/dev/console` 等特殊文件，以及只读文件的 copy-on-write 写。
* `/tmp`、`/var/log/nginx` 等目录可写，nginx 的 pid 文件和临时目录工作正常。

### 3.6 网络

* **virtio-mmio legacy** 网卡驱动（轮询、无中断）：RX/TX 各 64 个描述符，
  2 KiB 缓冲，处理 virtio-net 10 字节头部。
* **smoltcp 0.14** 提供 TCP/IP（IPv4、ARP、TCP），静态地址
  `10.0.2.15/24`，网关 `10.0.2.2`（QEMU slirp）。
* 在内核里模拟 Linux socket 语义：fd 表、非阻塞标志、`accept` 队列、
  `epoll` 兴趣表和水平触发就绪判断。
* 关键设计：smoltcp 的一个监听 socket 一次只能处理一个连接，所以每个
  `listen()` 预创建 24 个监听 socket 组成池；某个 socket 脱离 `LISTEN`
  后立刻把它移到 `accept` 队列并补充新的监听 socket，从而支持并发连接。

### 3.7 事件循环与阻塞

nginx 用 `epoll` + 非阻塞 socket。内核的阻塞型 syscall（`epoll_pwait`、
`recv`、`nanosleep`、`eventfd read`…）内部循环调用 `net::poll()`：
驱动 virtio RX、推进 smoltcp 状态机、收割监听池、清理关闭的连接，然后检查
就绪条件。这样单线程协作式调度即可同时服务多个连接。

### 3.8 完整性/安全边界

因为运行在单地址空间、无 MMU，内核在拷贝用户指针时做范围检查
（`0x8800_0000 … 0x1_0000_0000`），非法地址返回 `EFAULT`，避免内核直接崩溃。
除此之外没有进程隔离——本项目的目标是“把真实的 nginx 跑起来”，而不是做一个
安全的多用户内核。

---

## 4. 验证

在 `MacBook (Apple Silicon) + QEMU 11.1.2` 上：

* 单次请求：`HTTP/1.1 200 OK`，正文 211 字节，`Server: nginx/1.26.3`。
* 不存在路径：`404 Not Found`（nginx 生成）。
* 连续 100 次请求：100/100 成功。
* 20 个并发请求：19–20/20 成功（受监听池与 slirp 重传影响）。
* 30 个瞬时并发：约 23/30 成功；超出监听池的 SYN 会被丢弃/重置。
* `lsof -iTCP:8080`：`qemu-syst ... TCP *:8080 (LISTEN)`，即绑定所有网卡。

> 说明：并发上限来自“单监听端口 + 固定监听池”的实现取舍；作为演示已经足够，
> 继续提高只需增大池并处理 SYN 队列。

---

## 5. 目录结构

```
Cargo.toml            内核 crate（no_std，target riscv64gc-unknown-none-elf）
linker.ld             链接脚本，内核基址 0x8020_0000
rootfs.cpio           内嵌根文件系统（由 tools/mkrootfs.py 生成）
rootfs/               根文件系统内容（nginx、musl、配置、网页）
vendor/               vendor 的第三方 crate（smoltcp 等），支持离线构建
src/
  main.rs             入口、启动参数、模块装配
  arch.rs             CSR、TrapFrame、trap 入口、进入 U-mode
  trap.rs             异常分发（ecall -> syscall）
  syscall_nr.rs       asm-generic syscall 编号（自动生成）
  syscall.rs          Linux 系统调用实现
  elf.rs              ELF64 装载器
  user.rs             用户栈/auxv 构造、exec
  heap.rs             内核 free-list 分配器（GlobalAlloc）
  mm.rs               用户 mmap 区管理
  fs.rs               cpio + RAM overlay 文件系统、fd 表
  virtio.rs           virtio-mmio legacy 网卡驱动
  net.rs              smoltcp 集成、socket/epoll/eventfd/socketpair
  uart.rs, sbi.rs     串口与 SBI 关机
tools/
  prepare_rootfs.sh   从上游 Alpine 包重建 rootfs（含 nginx 校验和）
  mkrootfs.py         生成 newc cpio
build.sh, run.sh      构建 / 运行
```

## 6. 已知限制

* 单核、单进程（无 `fork`/`clone`/多线程），nginx 以 `master_process off` 运行。
* 无 MMU：没有用户态内存保护、按需分页和 COW。
* 仅 IPv4；无 IPv6、UDP 应用（socket 层只暴露 TCP）。
* 无磁盘/块设备，rootfs 内嵌且只读（RAM overlay 可写）。
* 网络收发为轮询，空闲时 CPU 占用较高；吞吐量远低于原生 Linux。
* `sendfile` 关闭；大文件通过普通读写传输。

这些限制都是为“在自制内核上运行真实 nginx”这一目标所做的明确取舍。
