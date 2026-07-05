---
name: ijiege-os-virtio-net-blocker
description: virtio-net 驱动阻塞——设备不处理 virtqueue 描述符
metadata:
  type: project
---

[[ijiege-os-project]] [[ijiege-os-milestone-real-linux-binary]]

截至 2026-07-06，virtio-mmio net 驱动卡住：设备**不处理任何 virtqueue 描述符**（TX/RX used 环始终为 0），尽管配置经反复验证正确。

**已验证正确:**
- 设备在 0x10008000（DTB 确认 virtio_mmio@10008000，中断 8），DeviceID=1，MAC=52:54:00:12:34:56
- Version=1（legacy）。feat0=0x39bf8064，feat1=0（无 VIRTIO_F_VERSION_1 → 不支持 modern）
- Status=0x7（ACK|DRIVER|DRIVER_OK），QueuePFN 回读匹配（队列已激活）
- 3-page 布局（VQ_SIZE=256）：desc@base, avail@base+4096, used@base+8192，全部页对齐，匹配 legacy 计算
- desc0: addr/len=52/flags=0/next=0 正确；avail: flags=0 idx=2 ring[0]=0 正确
- PLIC source 8 已启用，sie.SEIE 已置位，PLIC 区域已映射（4MB）
- gratuitous ARP 包内容正确

**但:** TX used 环 dump 全零，设备从未写入。RX 也无任何包。

**可能原因（未确认）:**
- QEMU 11.0 virtio-mmio 可能默认 modern，Version=1 是误读或怪异行为，但 feat1=0 又不支持 modern
- 某个时序/屏障问题未解决
- 可能需要不同的设备附加方式（`-device virtio-net-device` 是否真接到 net0）

**未尝试:**
- 对照已知可用的 virtio-mmio legacy 驱动逐行比对（如 xv6-riscv virtio.c）
- 用 QEMU monitor 确认设备接到了 net0
- 尝试 `-device virtio-net-pci`（需写 PCI 枚举驱动）

**内核其余部分已完成且可用:** Phase 1-7（UART/trap/Sv39 页表+堆/时钟抢占调度器/U-mode 进程+ELF 加载/Linux syscall 子集/run 真实 glibc 静态二进制/ initramfs VFS）。仅网络驱动阻塞 nginx 目标。
