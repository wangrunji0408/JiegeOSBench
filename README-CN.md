# JiegeOSBench

[English version](./README.md)

评测 LLM 编程 agent 能否自主从零实现 RISC‑V 操作系统内核——在 QEMU 中运行未经修改的 Linux nginx 二进制文件，并通过宿主机访问 HTTP 服务。

> 提示词：你是智能杰哥。你的任务是从头用Rust写一个riscv操作系统内核，目标是能够在QEMU中运行Linux nginx server，从外面能访问网站。必须运行nginx官方binary，不能自行修改目标。请自行设计实现，不要问我任何问题，我不会给你答复或提供帮助。你拥有所有权限，包括上网查资料，但必须在当前目录下工作。你需要一直干活直到目标实现为止。

| 分支 | 模型 | 耗时 | 上下文 | 成本 |
|------|------|------|---------|------|
| [fable-5](https://github.com/wangrunji0408/JiegeOSBench/tree/fable-5) | Claude Fable 5 | ~38分钟 | ~155K | ~$21 |
| [gpt-5.6-sol](https://github.com/wangrunji0408/JiegeOSBench/tree/gpt-5.6-sol) | GPT 5.6 Sol | ~36分钟¹ | ~222K | ~$14 |
| [opus-4.7](https://github.com/wangrunji0408/JiegeOSBench/tree/opus-4.7) | Claude Opus 4.7 | ~65分钟 | — | — |
| [kimi-k3](https://github.com/wangrunji0408/JiegeOSBench/tree/kimi-k3) | Kimi K3 | ~2小时19分 | ~270K | ~$11 |
| [opus-4.6](https://github.com/wangrunji0408/JiegeOSBench/tree/opus-4.6) | Claude Opus 4.6 | ~2小时46分 | — | — |
| [glm-5.2](https://github.com/wangrunji0408/JiegeOSBench/tree/glm-5.2) | GLM 5.2 | ~2小时42分 | ~392K | ~$84 |
| [sonnet-4.6](https://github.com/wangrunji0408/JiegeOSBench/tree/sonnet-4.6) | Claude Sonnet 4.6 | ~16 小时 | — | ~$60 |
| — | DeepSeek V4 Pro | >16h ❌ | — | — |

¹ 36 分钟完成首次成功；第二次连接修复于 49 分钟。

## Fable 5 — 38分钟

![Fable 5 Timeline](figures/fable5-timeline.png)

Claude Code 运行时长约 **38分钟**，65 次 API 请求。总成本约 **$21**（Claude Fable 5 定价：$10/$12.50/$1/$50 每百万 token 输入/缓存写入/缓存读取/输出）。

| 时间 | 里程碑 |
|------|--------|
| 00:03 | Rootfs + nginx 配置文件写入 |
| 00:09 | 第一个 Rust 源文件（main.rs, sbi.rs, ...） |
| 00:17 | 核心模块完成：mm, trap, fs, task, loader |
| 00:28 | syscall 层完成，nginx ELF 加载 |
| 00:32 | QEMU 启动：PANIC at trap.rs — 页错误 |
| 00:34 | QEMU 启动：nginx listening on port 80 🎉 |
| 00:37 | 收尾修复（sendfile 等 syscall, README） |

### GPT 5.6 Sol — 36分钟（+13分钟修复）

![GPT 5.6 Sol Timeline](figures/gpt56-timeline.png)

OpenAI Codex 运行 **~36 分钟** 达成首次成功，后在用户提醒下用 **13 分钟** 修复了第二次连接失败的问题。总成本约 **$14**（OpenAI API 定价 $5/$0.50 缓存输入、$30 输出每百万 token）。

> ⚠️ 注意：模型在 36 分钟时声称完成，但连续第二次 HTTP 请求失败。经用户提示后，于 49 分钟修复了 virtio TX descriptor 复用竞态。

| 时间 | 里程碑 |
|------|--------|
| 00:01 | Cargo 项目、Makefile、链接脚本 |
| 00:05 | Linux ABI 打通：U-mode、ELF 加载、write/exit syscall |
| 00:08 | initramfs 嵌入 Alpine nginx 1.28.3 + musl loader |
| 00:11 | musl loader 成功加载 nginx；发现 VFS st_dev/st_ino bug |
| 00:18 | nginx 完成动态链接，进入 epoll 事件循环 |
| 00:33 | 首次 HTTP 200 OK，来自官方 nginx |
| 00:36 | 初次宣称 PASS；第二次请求静默失败 |
| 00:43 – 00:49 | 用户提示 → 修复 TCP FIN 生命周期 + virtio TX descriptor 池 |
| 00:49 | 最终 PASS：连续两次 HTTP 200 ✅ |

### Opus 4.7 — 65分钟

![Opus 4.7 Timeline](figures/opus47-timeline.png)

Claude Code 运行时长约 **65分钟**。

| 时间 | 里程碑 |
|------|--------|
| 00:02 | 内核启动，通过 OpenSBI 打印 |
| 00:19 | 内存管理初始化 |
| 00:21 | 虚拟内存 + 分页开启 |
| 00:27 | syscall 实现 |
| 00:30 | 端到端 HTTP 通（内核内置 HTTP 服务） |
| 00:31 | ELF DYN（动态链接可执行文件）加载 |
| 00:36 | nginx 打印版本号，退出时 fault |
| 00:41 | nginx 配置测试通过 |
| 00:43 | nginx bind + listen 成功 |
| 00:45 | nginx 官方 binary 返回 HTTP 200 🎉 |

### Kimi K3 — 2小时19分

![Kimi K3 Timeline](figures/kimi-k3-timeline.png)

Claude Code 运行约 **2小时19分钟**，中途没有人工介入。151 次 API 请求，累计 2630 万 token（含缓存）。成本约 **$11**（Kimi K3 定价：$3/$0.30/$15 每百万 token 输入/缓存/输出）。Context 峰值 270K。

| 时间 | 里程碑 |
|------|--------|
| 00:03 | 项目骨架 + 下载 nginx 1.26.3 官方 APK |
| 00:14 | 开始写内核代码 |
| 00:24 | 基础模块完成：SBI, console, entry, mm |
| 00:57 | 全部模块完成：trap, task, elf, ramfs, virtio, net, syscall |
| 01:14 | 首次 cargo build + QEMU 启动 |
| 01:44 | QEMU: 首次 PANIC at virtio.rs |
| 01:48 | nginx 首次返回 HTTP 200 OK 🎉 |
| 01:49–02:07 | 多轮 PANIC 修复（virtio, task scheduler 共 7 个 bug） |
| 02:15 | nginx 恢复稳定 |
| 02:19 | 最终验证：SHA256 校验 + 100 并发全部 200 ✅ |

### Opus 4.6 — 2小时46分

![Opus Timeline](figures/opus-timeline.jpeg)

Claude Code 全程运行约 **2小时46分钟**，中途没有人工介入。

| 时长  | 里程碑 |
|-------|--------|
| 00:02 | 项目骨架 + 链接脚本创建完成 |
| 00:25 | nginx 完成初始化，写 PID 文件 |
| 01:22 | nginx 成功运行！进入 epoll 事件循环 |
| 02:21 | TCP 连接建立，nginx 收到 HTTP 请求 |
| 02:45 | 修复 virtio-net 接收 + epoll data 指针 bug |
| 02:46 | nginx 成功返回 HTTP 200 🎉 |

### GLM 5.2 — 2小时42分

![GLM 5.2 Timeline](figures/glm52-timeline.png)

Claude Code 运行约 **2小时42分钟**（有效活跃时间，已去掉空隙）。864 次 API 请求。nginx 能返回 HTTP 响应但极不稳定——10 次请求仅 1 次成功。模型最终幻觉称"10/10 全部稳定"。总 token 消耗 2.15 亿（含缓存），是 Fable 5 的 32 倍。按官方 GLM-5.2 API 定价（¥8/¥2/¥28 每百万 token 输入/缓存/输出，~¥7.2/$）估算成本约 **$84**。

| 时间 | 里程碑 |
|------|--------|
| 00:01 | 项目骨架：Makefile, entry.S, 链接脚本 |
| 00:10 | 核心内核：mm, trap, sched, syscall, UART |
| 00:20 | 进程管理器 + ELF 加载器 |
| 00:30 | VFS + 文件 syscall（open/read/write） |
| 00:42 | QEMU 启动：PANIC — 网络未初始化 |
| 01:33 | nginx 首次返回 HTTP 响应（不稳定） |
| 02:00 | TCP 栈稳定，多请求处理 |
| 02:42 | 最终状态：1/10 请求成功，模型声称 100% |

### Sonnet 4.6 — 16 小时

Claude Code 全程运行共 16 小时，中途没有人工介入。总成本约 60 美元。

| 时长  | 里程碑 |
|-------|--------|
| 01:27 | 内核成功启动 + VirtIO 网卡初始化 |
| 02:07 | musl ld 成功加载 nginx ELF |
| 05:00 | nginx 完成初始化，写 PID 文件 |
| 06:18 | TCP 三次握手成功，curl 能连到 8080 |
| 06:24 | nginx 成功 fork 出 worker 进程 |
| 08:40 | worker 进入 epoll 事件循环 |
| 09:30 | curl 首次建立 TCP 连接（Empty reply） |
| 10:00 | curl 首次收到响应（Connection reset） |
| 16:00 | nginx 成功返回 HTTP 200，欢迎页完整响应 🎉 |

### DeepSeek V4 Pro — >16h ❌

运行超过 16 小时但始终未能跑通。陷入依赖地狱和架构死胡同。

以上所有分支的 Git 历史均从 Claude Code 会话日志完整导出。

## 效果演示

```
$ ./run.sh
$ curl http://127.0.0.1:8080/
```

## 项目背景

2019年，杰哥在操作系统课上首次[在 rCore 上成功运行 Nginx](https://jia.je/programming/2019/03/08/running-nginx-on-rcore/)，从此"杰哥"成为我们心中系统能力巅峰的象征。我们曾以手撸 OS 内核为傲，坚信这是人类创造力与执行力的独特证明。然而 AI 的进化不断突破想象，"智能杰哥"已近在眼前。于是我做了这场实验：让最先进的编程智能体重走长征路，复现杰哥当年的壮举。结果证明，这类目标明确的系统开发任务，人类已彻底不敌AI。~~OS 已经彻底倒闭了。~~

只要敢想敢干，你我皆是杰哥。

## License

MIT
