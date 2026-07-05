---
name: ijiege-os-project
description: 智能杰哥 RISC-V 内核项目——目标与当前进度
metadata:
  type: project
---

项目: `/Users/wangrunji/Codes/iJiege-glm` —— 用 Rust 从头写 RISC-V 内核，最终目标在 QEMU 中跑 nginx 官方 binary 对外提供网站。不修改 nginx。

**架构决策:**
- QEMU virt 机器，OpenSBI 加载内核到 0x80200000。
- 用 nightly-2026-06-12 + `riscv64gc-unknown-none-elf` target + build-std。
- 无外部 crate（自己写 CSR 访问、页表、堆分配器、调度器）。
- bare-metal 工具链: homebrew `riscv64-elf-binutils` / `riscv64-elf-gcc`（注意前缀是 `riscv64-elf-*` 不是 `riscv64-unknown-elf-*`）。PATH: `/opt/homebrew/opt/riscv64-elf-binutils/bin` + `/opt/homebrew/opt/riscv64-elf-gcc/bin`。
- 构建: `rustup run nightly-2026-06-12 cargo build --release`；运行: `qemu-system-riscv64 -machine virt -m 128M -nographic -bios default -serial mon:stdio -kernel target/.../kernel`。

**关键陷阱:**
- LLVM 汇编器里 `#` 是注释符，`#define` 不生效——用 `.set`。
- trap.S 的 TrapContext 偏移必须与 Rust 结构体严格对应（曾因 x[31] 与 sepc 同偏移导致 scause 读成 0）。
- U-mode 用户页有 U 位，S-mode 内核访问需 sstatus.SUM=1（在每个 TrapContext.sstatus 里设），否则 write syscall 读用户 buffer 时缺页。
- 工作目录会因 `cd` 改变，运行 QEMU 前确保在 repo 根。

**进度（截至 2026-07-05）:**
- Phase 1-5 完成：UART、trap、Sv39 页表+堆、时钟抢占调度器、U-mode 进程+ELF 加载器。手写汇编用户程序（Linux ecall ABI）能 write+exit。
- 待办（巨大）: Phase 6 Linux syscall 子集、Phase 7 virtio-blk+fs+rootfs、Phase 8 virtio-net+TCP/IP+socket、Phase 9 跑 nginx。
- 现实: nginx 动态链接 glibc，需实现数百 syscall + TCP 栈 + 驱动，等同重写 Linux。会持续推进并如实报告每步状态。
