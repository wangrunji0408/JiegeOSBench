# Jiege OS

Jiege OS 是从零用 Rust 编写的 RV64 内核。它在 QEMU `virt` 机器上直接运行未修改的
Linux RISC-V nginx 动态链接二进制，并通过 virtio-net 向宿主机提供 HTTP 服务；运行时
不包含 Linux 内核。

## 验收

依赖：Rust（含 `riscv64gc-unknown-none-elf` target）、Zig、GNU Make、curl 和
`qemu-system-riscv64`。

```sh
make test
```

成功输出：

```text
PASS: official nginx served http://127.0.0.1:8080/
```

也可以执行 `make run`，然后在另一终端访问：

```sh
curl --noproxy '*' http://127.0.0.1:8080/
```

## 目标程序来源

构建脚本从 Alpine Linux v3.22 官方 riscv64 仓库下载 `nginx-1.28.3-r4.apk` 及其原始
musl/OpenSSL/PCRE2/zlib 依赖。nginx 可执行文件没有补丁或重链接。

- APK SHA-256: `9a66d023a0654306eb848264b35b8537135a826677ef492fa7476c92e70069c3`
- `/usr/sbin/nginx` SHA-256: `40cf404d4aa6a275c8fc43cd571202323cae0f71717902ae1370241f2148ffc9`

## 内核组成

- OpenSBI S-mode 启动、UART、异常入口和完整 RV64 用户上下文
- Sv39 页表、物理页分配、PIE/ELF 加载和 Linux 初始栈/auxv
- 内存 initramfs、文件描述符和 nginx 所需 Linux RV64 系统调用
- virtio-net split virtqueue，以及单连接 ARP/IPv4/TCP 数据路径
- socket、epoll、sendfile 与 nginx 文件服务链路

`scripts/fetch-rootfs.sh` 固定并校验可复现载荷来源；首次干净构建会下载约 3.3 MiB
的软件包。内核和嵌入式 initramfs 构建完成后约 7 MiB。
