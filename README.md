# iJiege-k3：从零用 Rust 编写的 RISC-V 操作系统内核

一个用 Rust 从零实现的 RISC-V 64 操作系统内核，能够在 QEMU 中运行**官方 nginx 二进制**（Alpine Linux riscv64 仓库的 nginx 1.26.3，动态链接 musl），并通过宿主机浏览器/curl 访问其提供的网站。

## 运行

```bash
./run.sh
```

然后在宿主机：

```bash
curl http://localhost:8080/
# HTTP/1.1 200 OK
# Server: nginx/1.26.3
# ... Welcome to nginx!
```

依赖：`qemu-system-riscv64`、Rust nightly（见 `rust-toolchain.toml`，自动下载 riscv64gc-unknown-none-elf target）。

## 架构

```
┌─────────────────────────────────────────────┐
│  nginx 1.26.3 (官方 binary, PIE + musl ldso) │  用户态
├─────────────────────────────────────────────┤
│  Linux ABI: ~120 个 syscall                  │
│  (fs/mm/process/socket/epoll/pipe/eventfd)   │
├──────────┬───────────┬──────────┬───────────┤
│ 进程/调度 │ 内存管理   │ ramfs    │ 网络栈     │
│ fork/exec │ Sv39 页表  │ (tar解包) │ smoltcp   │
│ 抢占式    │ 帧分配/堆  │ 符号链接 │ TCP socket │
├──────────┴───────────┴──────────┴───────────┤
│ trap (trampoline) │ virtio-mmio net 驱动     │
├─────────────────────────────────────────────┤
│  OpenSBI (M-mode)  →  QEMU virt (riscv64)    │
└─────────────────────────────────────────────┘
```

### 已实现的主要组件

- **启动与 trap**：OpenSBI 引导；trampoline 式 trap 进入/返回（`__alltraps`/`__restore` 映射到所有地址空间共享的高地址 `TRAMPOLINE`）；SBI 控制台、定时器（1ms tick 抢占式调度）。
- **内存管理**：Sv39 页表；内核恒等映射（1GiB 巨页）+ trampoline 映射；物理帧分配器（bump + 回收栈）；64MiB 内核堆（buddy allocator）；用户地址空间区域管理（mmap/munmap/mprotect/brk，含区域分裂）；fork 时深拷贝地址空间。
- **进程**：ELF64 加载（ET_EXEC / ET_DYN PIE），支持 `PT_INTERP` 动态链接器（ld-musl）加载与完整 auxv/argv/envp 初始栈；`fork`/`execve`/`wait4`/`exit_group`；内核栈顶页映射到 `TRAP_CONTEXT` 供 trap 上下文在用户页表下访问。
- **文件系统**：编译期内嵌 `rootfs.tar`，启动时解包为可写 ramfs；支持目录、符号链接（含 ld-musl 的 libc 软链）、`/dev/null|zero|urandom` 特殊文件、`/proc/self/exe`。
- **网络**：virtio-mmio modern (v2) 网卡驱动（split virtqueue，轮询模式）；smoltcp TCP/IP（静态 IP 10.0.2.15/24，网关 10.0.2.2）；socket/bind/listen/accept4/recv/send/epoll/poll/select；**监听 socket 池**（64 个 smoltcp socket 同端口监听）实现 accept backlog，支撑高并发连接。
- **其他**：pipe、eventfd、AF_UNIX socketpair、getrandom、rt_sigaction（保存不投递）、prlimit、uname 等。

### rootfs

`rootfs/` 由 Alpine Linux v3.21 官方 riscv64 软件包解包而成（`pkgs/` 中的 .apk）：

- `usr/sbin/nginx` — nginx 1.26.3 官方构建（未做任何修改）
- `lib/ld-musl-riscv64.so.1`、`usr/lib/lib{pcre,ssl,crypto,z}*` — 运行依赖
- `etc/nginx/nginx.conf` — 配置为 `daemon off; master_process off;`（单进程前台运行，作为 PID 1 init）
- `sbin/init -> /usr/sbin/nginx`

重新打包：`cd rootfs && tar --format=ustar -cf ../rootfs.tar .`

### QEMU 网络

`-netdev user,hostfwd=tcp::8080-:80`（slirp 用户态网络，宿主机 8080 → 客户机 80），`-global virtio-mmio.force-legacy=false` 强制 modern virtio。

## 开发过程中解决的关键问题

1. **bss 清零踩栈**：`clear_bss` 在 Rust 中清零包含 boot_stack 的 bss 段，把自己正在使用的栈清零，造成布局相关的随机启动失败。改为入口汇编在建栈前清零。
2. **errno 符号**：ramfs 层返回正 errno、syscall 层透传，导致 openat 失败返回 +2 被 musl 当作合法 fd，stderr 被误关。统一为负 errno。
3. **epoll_event 布局**：riscv64 的 `struct epoll_event` 非 packed（16 字节，data 在偏移 8），与 x86-64 的 12 字节 packed 不同，写错布局导致 nginx 拿到截断指针崩溃。
4. **CLOCK_REALTIME 从 0 开始**：nginx 的 `ngx_time_update` 在 `tp->sec == sec` 时提前返回，启动初期 tv_sec=0 导致时间字符串指针永远为 NULL，`ngx_log_error_core` memcpy 空指针崩溃。加上真实时间偏移。
5. **smoltcp 单监听 socket 无 backlog**：并发 SYN 被丢弃/RST。用监听 socket 池解决；并修复重复 listen 导致的池泄漏（SYN 被分派到泄漏 socket 永远不被 accept）。
6. **virtio RX 描述符泄漏**：recv 重投递时只分配不释放，32 个包后耗尽。改为复用同一描述符。
7. **close 不清 epoll 兴趣表**：fd 复用后 epoll_ctl ADD 返回 EEXIST，nginx 关连接。
