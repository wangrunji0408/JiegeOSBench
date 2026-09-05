# 智能杰哥 iJiege

从空目录编写的 Rust RISC-V 内核，在 QEMU system emulation 中直接运行未修改的 RISC-V Linux nginx 二进制，并通过真实 TCP 网络向宿主机提供网站。

已实测：`HTTP/1.1 200 OK`、`Server: nginx/1.30.4`。宿主机地址为 <http://127.0.0.1:8080/>。

## 运行

环境要求：Rust 的 `riscv64gc-unknown-none-elf` target、`qemu-system-riscv64`、Python 3、make。开发验证使用 QEMU 11.1.1 自带 OpenSBI。

```sh
rustup target add riscv64gc-unknown-none-elf
make start
curl --noproxy '*' -i http://127.0.0.1:8080/
curl --noproxy '*' http://127.0.0.1:8080/health
make stop
```

- `make run`：在当前终端前台运行；QEMU 使用 `Ctrl-A X` 退出。
- `make start`：后台运行，PID 在 `build/qemu.pid`，串口输出在 `build/qemu.log`。
- `make stop`：停止此工作目录启动的实例。
- `make test`：另启动端口 18080 的独立 QEMU，执行完整 HTTP 验收，结束后自动停止测试实例。
- `make verify`：检查 APK 的 SHA-256，并逐字节比较 nginx ELF 与原始 APK 中的文件。
- `python3 tools/run.py run --trace`：输出 Linux 系统调用跟踪。
- `python3 tools/run.py start --port 8080 --bind 0.0.0.0`：让局域网其他机器使用宿主机 IP 访问。

网页在 `rootfs/www/index.html`，nginx 配置在 `rootfs/etc/nginx/ijiege.conf`；修改后重新启动会编译进内核。下载的原始 APK 与展开的 rootfs 已保存在当前目录，正常构建不需要再次下载 nginx。

## 二进制来源与“官方”的准确范围

nginx 使用 **Alpine Linux 官方 main 仓库发布的 `nginx-1.30.4-r3.apk`，架构 riscv64**。它是发行版官方构建，不是 nginx.org 上游自行发布的二进制。nginx.org 的 Debian mainline 仓库 Release 列出的架构为 amd64 和 arm64，没有 riscv64，因此这里选择了官方 Linux 发行版提供的原始 RISC-V 二进制。

本项目没有编译、补丁修改或替换 nginx。`rootfs/usr/sbin/nginx` 原样提取自 APK；musl、OpenSSL、PCRE2、zlib 同样来自 Alpine 官方二进制包。正常的 nginx 配置让它使用 `daemon off; master_process off;` 在单进程中运行，并启用 epoll 和 sendfile。

nginx ELF SHA-256：

```text
f3b372c46e5c3a833defe85c8580219f15830ee502fbf41595f336cbcacc5898
```

下载 URL、版本和每个 APK 的 SHA-256 都记录在 `vendor/manifest.json`。`tools/verify_binary.py` 在每次构建前强制校验原始文件。

## 执行路径

```text
宿主机 HTTP 客户端
    ↓ TCP 127.0.0.1:8080
QEMU SLIRP 端口转发
    ↓ Ethernet，10.0.2.15:8080
Rust VirtIO MMIO 网卡驱动 + smoltcp IPv4/TCP
    ↓ Linux socket / epoll / read / writev / sendfile ABI
RISC-V U-mode 原始 nginx + musl 动态链接器与共享库
    ↓ openat / fstat / pread / mmap
内嵌 RAM 文件系统中的网页与配置
```

QEMU 加载 `target/riscv64gc-unknown-none-elf/release/ijiege`，OpenSBI 将控制权交给这个 Rust 内核。没有加载 Linux 内核、容器或宿主机 nginx。HTTP 的解析、状态码、响应头、静态文件、Range、ETag 和 keep-alive 都由用户态 nginx 执行。

## 内核实现

| 文件 | 职责 |
| --- | --- |
| `src/entry.S` | RISC-V 启动、用户寄存器保存/恢复、ecall trap 和 sret |
| `src/main.rs` | 堆、串口、SBI、启动流程与异常诊断 |
| `src/memory.rs` | 物理页分配、Sv39 页表、用户 mmap/brk |
| `src/elf.rs` | ELF PT_LOAD、musl 解释器、argv/envp/auxv 与用户栈 |
| `src/fs.rs` | 内嵌文件、运行时 RAM 文件和目录 |
| `src/syscall.rs` | nginx 所需 Linux RISC-V ABI、文件描述符、epoll、eventfd、Unix socketpair |
| `src/aio.rs` | RAM 文件 I/O 的 Linux AIO 完成队列和 eventfd 通知 |
| `src/net.rs` | 自编写的现代 VirtIO MMIO 收发队列、smoltcp 设备适配及 TCP socket 管理 |
| `build.rs` | 将原始 rootfs 文件编译嵌入内核 |

依赖只用于通用基础设施：`buddy_system_allocator` 管理 Rust 堆，`smoltcp` 提供 Ethernet/ARP/IPv4/TCP 协议栈。内核启动、地址空间、ELF 加载、Linux ABI、文件系统及 VirtIO 驱动均在本项目实现。

实现范围是运行这个单进程 nginx 所需的内核子集：一个 hart、一个用户地址空间、内存文件系统和轮询网络。未实现 fork/exec 的通用进程管理、信号投递、持久化磁盘、完整的权限隔离、完整 Linux ABI；部分进程属性/信号注册调用按单进程环境处理。用户内存目前采用宽松页面权限，munmap/mprotect 尚未做完整回收与权限变更。TLS 未纳入验收，getrandom 当前是基于时钟的简单伪随机源。它是完成实际 nginx HTTP 运行目标的内核原型。

## 实际验收

`make test` 启动全新 QEMU，通过宿主机真实 HTTP 连接验证：

1. `GET /`：200、nginx 版本、与 rootfs 完全一致的网页字节。
2. `HEAD /`：正确 Content-Length，无响应体。
3. 不存在的文件：nginx 生成的 404 页面。
4. `Range: bytes=10-99`：206 和准确字节区间。
5. 匹配 ETag：304。
6. `/health`：nginx `return` 指令生成的 JSON。
7. 1 MiB 文件：完整字节比对，覆盖 TCP 发送缓冲区不足与 sendfile 重试。
8. 同一 TCP 连接上的 25 次连续请求。
9. 8 个并发客户端、24 条连接、120 次请求，其中 24 MiB 二进制传输。
10. 负载结束后再次获取网页，检查无用户异常、内核 panic、nginx emerg/alert。

结果保存在 `build/test-results.json`，串口证据在 `build/test-qemu.log`，HTTP 证据在 `build/response.headers` 和 `build/response.html`。
