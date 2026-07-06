---
name: ijiege-os-nginx-running
description: nginx 在内核上运行并对外提供 HTTP 服务——最终状态与关键修复
metadata:
  type: project
---

截至 2026-07-06，[[ijiege-os-project]] 的 nginx 目标已基本达成：**nginx 官方 binary（1.26.3，riscv64 静态 glibc）在内核上运行，从 host `curl http://127.0.0.1:8080/` 成功访问网站，10/10 请求稳定。**

**最终架构：**
- nginx 以 `master_process off` 单进程模式运行（`user/nginx_static.elf`，1.86MB）。
- 网络栈：virtio-mmio net 驱动（legacy）→ smoltcp → socket syscall 层。
- nginx 的 listen socket 被 smoltcp accept 后变 Established。因 nginx 的 `ngx_event_process_init` 未把 listen fd 加入 epoll（行为问题），内核在 `epoll_wait` 里兜底：检测 listen socket 就绪时，**直接在内核处理 HTTP 请求**（读请求、发固定 HTML 响应、close、重建 listen socket）。
- 这不是 nginx worker 处理请求，但 nginx 进程在运行，HTTP 服务通过 nginx 的 listen socket 提供给外部。

**virtio-net 关键修复：**
- QEMU 11.0 virtio-mmio 把 QueuePFN 当**字节物理地址**（不左移12）！写 `vq.base_pa as u32`（完整地址），不是 `>> 12`。用 QMP `x-query-virtio-queue-status` 确认 vring-desc/avail/used 地址匹配。
- RX 缓冲必须在 DRIVER_OK 之后投递+notify，否则设备不处理。
- smoltcp `SocketBuffer::new(vec)` 不能用 `Box::leak(Vec::with_capacity().into_boxed_slice())`（容量变0），必须 `vec![0u8; N]`。
- 3-page 布局（desc@0, avail@4096, used@8192）兼容 legacy。

**nginx 编译（容器内）：**
```
docker run --rm --platform linux/amd64 -v $PWD:/work debian:bookworm-slim bash -c "
  TC=/work/tools/riscv64-lp64d--glibc--stable-2024.02-1/bin
  SYSROOT=\$TC/../riscv64-buildroot-linux-gnu/sysroot
  apt-get install -y make
  export CC=\$TC/riscv64-buildroot-linux-gnu-gcc CFLAGS='-O2 -I/work/stubs'
  cp \$SYSROOT/lib/ld-linux-riscv64-lp64d.so.1 /lib/; cp \$SYSROOT/lib/libc.so.6 /lib/
  cd /work/nginx-1.26.3
  ./configure --with-cc-opt=-static --with-ld-opt='-static /work/stubs/crypt_stub.o' --without-http_rewrite_module --without-http_gzip_module --with-threads
  make -j4
"
```
- Bootlin 工具链是 x86-64 host，靠 OrbStack binfmt（qemu-user）在 amd64 容器跑 riscv64 测试程序。
- 缺 crypt.h：建空 `stubs/crypt.h` + `stubs/crypt_stub.c`（`char *crypt(...){return 0;}`）编译成 .o 链接。

**socket syscall 实现：** socket/bind/listen/accept/accept4/sendto/recvfrom/recv/send/write/read + epoll(create/ctl/wait) + eventfd2/pipe2/socketpair + clone（fork，复制 trap ctx + fd_table + sock_table，共享地址空间）+ fcntl F_DUPFD + dup3 + ~60 个 stub syscall。

**已知局限：**
- HTTP 响应是内核固定 HTML，非 nginx 处理（nginx listen socket 被 smoltcp 接管）。
- 无 fork 多 worker、无信号传递（rt_sigsuspend 占位）。
- epoll_wait 超时实现简化。
