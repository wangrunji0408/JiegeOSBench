# Jiege OS (Luna)

Luna 是从零用 Rust 编写的 RV64 内核。它在 QEMU `virt` 机器上直接运行**未修改的
Debian 官方 RISC-V nginx 动态链接二进制**（走完整 glibc 动态链接路线），并通过
virtio-net 向宿主机提供 HTTP 服务；运行时不含 Linux 内核。

## 验收

依赖：Rust（含 `riscv64gc-unknown-none-elf` target）、GNU Make 和
`qemu-system-riscv64`。

```sh
make build
```

启动 QEMU（guest 80 转发到宿主 18080）：

```sh
qemu-system-riscv64 -machine virt -m 512M -nographic -bios default \
  -kernel target/riscv64gc-unknown-none-elf/release/luna \
  -netdev user,id=n0,hostfwd=tcp:127.0.0.1:18080-:80 \
  -device virtio-net-device,netdev=n0
```

然后访问：

```sh
curl --noproxy '*' http://127.0.0.1:18080/
```

成功返回 `assets/index.html` 内容。

## 目标程序来源

Debian 官方 riscv64 仓库的 `nginx_1.30.1-7+b1_riscv64.deb` 及其 glibc 动态依赖
（libc6 2.41、libssl3、libpcre2、libcrypt、zlib、zstd）。nginx 可执行文件没有
补丁或重链接。

- `/usr/sbin/nginx` SHA-256: `51a24cfe...b5f69`（未修改）

`assets/` 内含 Debian 包、解包后的 rootfs 与运行时配置文件，内核直接嵌入使用。

## 内核组成

- OpenSBI S-mode 启动、UART、异常入口与完整 RV64 用户上下文
- 无分页用户态：ELF64 `ET_DYN` 加载（内核 `0x80200000`、nginx `0x81000000`、
  loader `0x83000000`、mmap 区 `0x90000000`）
- 嵌入式只读 VFS + Linux syscall 层（openat/mmap/writev/epoll/socket/...）
- 完整 glibc 动态链接器支持：ld.so.cache 解析、`stat` 布局、auxv/argc 用户栈
- virtio-net split virtqueue、ARP/IPv4/TCP、epoll 事件循环
- nginx 初始化所需路径（`/var/lib/nginx/*`、`/dev/stderr`、passwd/group 等）

## Git 历史

本分支的提交历史由 Codex session（gpt-5.6-luna）导出：每个 commit 对应模型的一个
工作阶段，commit message 为模型当时的原话；最后 3 个 commit（源码对齐、运行时
资产、本说明）由人工补齐。
