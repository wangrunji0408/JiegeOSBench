# iJiege — 一个能跑 nginx 的 Rust RISC-V 操作系统内核

ijiege 是一个从零开始用 Rust 编写的 RISC-V (RV64) 内核，目标是在 QEMU 中
运行 **未经修改的 Alpine Linux 官方 nginx 二进制**，并让外部主机通过 HTTP 访问它。

```
$ make run
$ curl http://127.0.0.1:8080/
<!DOCTYPE html>
<html>
<head><title>Hello from Rust RISC-V kernel</title></head>
<body>
<h1>It works! nginx 1.28.3 on a hand-written Rust RISC-V kernel</h1>
...
```

## 它是什么

- **纯 Rust、no_std** 内核（唯一外部依赖：[smoltcp](https://github.com/smoltcp-rs/smoltcp) 网络栈）
- 运行 **官方 Alpine Linux riscv64 的 nginx 1.28.3 apk 包**（动态链接 musl + libssl + libcrypto + libpcre2 + zlib，全部为发行版原版二进制）
- 不模拟 Linux：内核直接实现 Linux/riscv64 的 syscall ABI（内存、文件、socket、epoll、信号、时间等约 90 个系统调用）

## 系统结构

```
kernel/src/
├── start.rs        启动汇编（OpenSBI → _start）
├── main.rs         内核入口：内存/网络/trap 初始化 → 启动 nginx
├── sbi.rs          SBI 调用（timer/shutdown）
├── uart.rs         NS16550 串口驱动
├── dtb.rs          设备树解析（内存大小）
├── pmm.rs          物理页帧分配器（+ 内核自旋锁）
├── heap.rs         内核堆分配器
├── page.rs         SV39 三级页表（4K/2M 页）
├── trap.rs         用户态/内核态上下文切换（含浮点保存恢复）
├── proc.rs         进程（VMA、fd 表、brk/mmap 地址空间）
├── elf.rs          ELF64 加载器（静态/动态、PIE、Linux 栈+auxv 构造）
├── vfs.rs          内嵌 tar rootfs（只读）+ tmpfs 可写层 + 符号链接
├── syscall.rs      Linux syscall 分发（riscv64 编号表自动生成自 musl）
├── errno.rs        Linux errno
└── net/
    ├── virtio_net.rs  virtio-mmio 现代模式网卡驱动（轮询、多队列）
    ├── stack.rs       smoltcp 集成（TCP/ARP/IPv4，10.0.2.15 静态配置）
    └── socket.rs      socket/epoll/eventfd 的 Linux 语义实现

rootfs/             nginx 与依赖库（官方 apk 解包）+ 配置
Makefile            构建 & 运行
```

## 关键设计

- **单进程事件驱动模型**：nginx 以 `master_process off; daemon off;` 单进程模式运行，
  IO 等待在 syscall 内部通过「poll 网络栈 + 自旋计时」完成，无需调度器与 fork。
- **用户页表内恒等映射内核区**：trap 发生时不切换 satp，内核可直接运行/访问用户内存（SUM=1）。
- **内核全程关中断**：等待用自旋（QEMU TCG 下 SIE=0 时 wfi 不会因 timer pending 唤醒）。
- **rootfs 以 tar 形式链接进内核镜像**：只读层零拷贝（`&'static [u8]`），写操作落入 tmpfs。
- **epoll（水平触发）**：每次 `epoll_wait` 先轮询 smoltcp，再扫描注册的 fd 生成事件。
- **listen 池**：每个监听端口由多个 smoltcp TCP socket 同时 listen，accept 时取走
  Established 的那个并补充新 socket，支撑并发连接。

## 运行

依赖：rust (riscv64gc-unknown-none-elf target)、qemu-system-riscv64 ≥ 8、tar。

```sh
make run    # 构建内核并启动 QEMU（host 8080 → guest 80）
make kernel # 只构建
```

QEMU 参数（见 Makefile）中的 `-global virtio-mmio.force-legacy=false` 必需：
它让 virtio 网卡进入 modern 模式。

## 验证结果

- 静态 musl busybox 1.37：`echo` 等正常退出 ✓
- 动态 musl busybox（ld.so + libc）✓
- **nginx 1.28.3（Alpine 官方包，动态链接 4 个共享库）**：
  - 启动、读取配置、监听 80 端口 ✓
  - curl 连续与并发请求均 HTTP 200（延迟 ~12ms）✓
  - sendfile 传输静态页面 ✓
  - 访问日志写入 tmpfs ✓

## 已知限制

- 单进程：不支持 fork/clone（nginx 需以 `master_process off` 配置运行）
- 网络仅 TCP（IPv4）；无 UDP/DNS
- 未启用中断，CPU 在 IO 等待时自旋
- 信号只做注册记录，不实际投递
