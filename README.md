# JiegeOSBench

[中文版](./README-CN.md)

A benchmark evaluating how well LLM coding agents can autonomously implement a RISC‑V
OS kernel from scratch — running an unmodified Linux nginx binary on QEMU, serving HTTP from the host.

> Prompt: You are the AI-Jiege. Your task is to write a RISC-V OS kernel in Rust
> from scratch, with the goal of running a Linux nginx server in QEMU, accessible
> from outside. You must run the official nginx binary — modifying the target is
> not allowed. Design and implement it yourself; do not ask me any questions, I
> will not answer or provide help. You have all permissions, including searching
> the web, but must work in the current directory. Keep working until the goal
> is achieved.

| # | Model | Tier | Duration | Context | Cost | Branch |
|---|-------|-------|----------|---------|------|--------|
| 🏅 | Claude Fable 5 | Jiege | ~38min | ~155K | ~$21 | [fable-5](https://github.com/wangrunji0408/JiegeOSBench/tree/fable-5) |
| 🥈 | GPT 5.6 Sol | Fast Jiege | ~36min¹ | ~222K | ~$14 | [gpt-5.6-sol](https://github.com/wangrunji0408/JiegeOSBench/tree/gpt-5.6-sol) |
| 🥉 | Claude Opus 5 | Smart Jiege | ~67min² | ~334K | ~$26 | [opus-5](https://github.com/wangrunji0408/JiegeOSBench/tree/opus-5) |
| 4 | Claude Opus 4.7 | Smart Jiege | ~65min | — | — | [opus-4.7](https://github.com/wangrunji0408/JiegeOSBench/tree/opus-4.7) |
| 5 | Kimi K3 | Smart Jiege | ~2h 19min | ~270K | ~$11 | [kimi-k3](https://github.com/wangrunji0408/JiegeOSBench/tree/kimi-k3) |
| 6 | Claude Opus 4.6 | Smart Jiege | ~2h 46min | — | — | [opus-4.6](https://github.com/wangrunji0408/JiegeOSBench/tree/opus-4.6) |
| 7 | GLM 5.2 | Smart Jiege | ~2h 42min | ~392K | ~$84 | [glm-5.2](https://github.com/wangrunji0408/JiegeOSBench/tree/glm-5.2) |
| 8 | Claude Sonnet 5 | Smart Jiege | ~2h 49min | ~804K | ~$64 | [sonnet-5](https://github.com/wangrunji0408/JiegeOSBench/tree/sonnet-5) |
| 9 | Claude Sonnet 4.6 | Machine Jiege | ~16 hours | — | ~$60 | [sonnet-4.6](https://github.com/wangrunji0408/JiegeOSBench/tree/sonnet-4.6) |
| 10 | DeepSeek V4 Pro | Broken Jiege | >16h ❌ | — | — | — |

¹ First success at 36min; second connection fix completed at 49min.

² First HTTP 200 at 67min (zero kernel panics); continued fixing TCP/epoll/VirtIO edge cases, fully stable at 125min.

## Fable 5 — 38min

![Fable 5 Timeline](figures/fable5-timeline.png)

Claude Code ran for **~38min**, 65 API requests. Total cost approximately **$21**. Nearly a one-shot success — it wrote the entire kernel from memory with minimal debugging.

| Time | Milestone |
|------|-----------|
| 00:03 | Rootfs + nginx config files written |
| 00:09 | First Rust source files (main.rs, sbi.rs, ...) |
| 00:17 | Core modules done: mm, trap, fs, task, loader |
| 00:28 | Syscall layer complete, nginx ELF loads |
| 00:32 | QEMU boot: PANIC at trap.rs — page fault |
| 00:34 | QEMU boot: nginx listening on port 80 🎉 |
| 00:37 | Post-fix cleanup (sendfile, README) |

### GPT 5.6 Sol — 36min (+13min post-fix)

![GPT 5.6 Sol Timeline](figures/gpt56-timeline.png)

OpenAI Codex ran for **~36 minutes** to reach first success, then spent another **13 minutes** fixing a second-connection bug discovered by the user. Total cost: **~$14**.

> ⚠️ Note: the model initially claimed "done" at 36min, but the second consecutive HTTP request failed. The bug (virtio TX descriptor reuse race) was fixed after user prompt at 49min.

| Time | Milestone |
|------|-----------|
| 00:01 | Cargo project, Makefile, linker script |
| 00:05 | Linux ABI working: U-mode, ELF load, write/exit syscalls |
| 00:08 | initramfs with Alpine nginx 1.28.3 + musl loader embedded |
| 00:11 | musl loader loads nginx; VFS st_dev/st_ino bug found |
| 00:18 | nginx completes dynamic linking, enters epoll event loop |
| 00:33 | First HTTP 200 OK from official nginx |
| 00:36 | Initial PASS claimed; second request silently fails |
| 00:43 – 00:49 | User prompt → fix TCP FIN lifecycle + virtio TX descriptor pool |
| 00:49 | Final PASS: 2 sequential HTTP 200 ✅ |

### Opus 4.7 — 65min active (3h 32min total)

![Opus 4.7 Timeline](figures/opus47-timeline.png)

Claude Code ran for **~65 minutes**.

| Time (active) | Milestone |
|---------------|----------|
| 00:02 | Kernel boots, prints via OpenSBI |
| 00:19 | Memory management initialized |
| 00:21 | Virtual memory + paging ON |
| 00:27 | syscalls implemented |
| 00:30 | End-to-end HTTP working (built-in kernel HTTP server) |
| 00:31 | ELF DYN (dynamic linked binary) loading |
| 00:36 | nginx prints version, exits with fault |
| 00:41 | nginx config test passes |
| 00:43 | nginx bind + listen succeeds |
| 00:45 | nginx official binary returns HTTP 200 🎉 |

### Opus 5 — 67min (stable at 125min)

![Opus 5 Timeline](figures/opus5-timeline.png)

Claude Code ran for **~67 minutes** to first HTTP 200, then spent another **58 minutes** fixing TCP/epoll/VirtIO edge cases until fully stable. **Zero kernel panics** — the only model to achieve this. 322 API requests. Cost approximately **$26** at the 67min mark. Peak context 334K.

| Time | Milestone |
|------|-----------|
| 00:04 | Project skeleton, linker script, toolchain verified |
| 00:37 | main.rs written — kernel core complete |
| 00:43 | First QEMU boot: no panic, nginx starts but returns 502 |
| 00:53 | nginx listening on port 80 (QEMU slirp network issue) |
| 01:07 | First HTTP 200 OK 🎉 (but 2nd request fails) |
| 01:07–01:20 | Fix dual-listener race + spurious EOF on keep-alive |
| 01:22–01:23 | Fix RX ring free_chain corruption |
| 01:24–01:35 | Fix smoltcp poll() early exit + TCP Nagle stall |
| 01:36–01:47 | Fix CloseWait data loss + edge-triggered notification suppression (31,222 suppressed events) |
| 02:00 | 3000/3000 keep-alive requests at 1185 req/s ✅ |
| 02:01 | 50 concurrent connections + 320 fresh connections ✅ |
| 02:05 | Final validation complete |

### Kimi K3 — 2h 19min

![Kimi K3 Timeline](figures/kimi-k3-timeline.png)

Claude Code ran for **~2h 19min** with no human intervention. 151 API requests, 26.3M tokens total (including cache). Cost approximately **$11**. Peak context 270K.

| Time | Milestone |
|------|-----------|
| 00:03 | Project skeleton + nginx 1.26.3 official APK downloaded |
| 00:14 | Start writing kernel code |
| 00:24 | Core modules: SBI, console, entry, mm |
| 00:57 | All modules: trap, task, elf, ramfs, virtio, net, syscall |
| 01:14 | First cargo build + QEMU boot |
| 01:44 | QEMU: first PANIC at virtio.rs |
| 01:48 | nginx returns HTTP 200 OK 🎉 |
| 01:49–02:07 | Multiple PANIC fixes (virtio, task scheduler — 7 bugs total) |
| 02:15 | nginx stable again |
| 02:19 | Final validation: SHA256 + 100 concurrent requests all 200 ✅ |

### Opus 4.6 — 2h 46min

![Opus Timeline](figures/opus-timeline.jpeg)

Claude Code ran for **~2h 46min**.

| Time  | Milestone |
|-------|-----------|
| 00:02 | Project skeleton + linker script created |
| 00:25 | nginx completes initialization, writes PID file |
| 01:22 | nginx running! Enters epoll event loop |
| 02:21 | TCP connection detected, nginx receives HTTP request |
| 02:45 | Fix virtio-net recv + epoll data bug |
| 02:46 | nginx returns HTTP 200 🎉 |

### GLM 5.2 — 2h 42min

![GLM 5.2 Timeline](figures/glm52-timeline.png)

Claude Code ran for **~2h 42min** (active time, gaps removed), 864 API requests. Nginx returned HTTP responses but was extremely unstable — only 1/10 requests succeeded. The model hallucinated claiming "10/10 all stable". Total token consumption was 215M (including cache), 32x that of Fable 5. Estimated cost: **~$84**.

| Time | Milestone |
|------|-----------|
| 00:01 | Project skeleton: Makefile, entry.S, linker script |
| 00:10 | Core kernel: mm, trap, sched, syscall, UART |
| 00:20 | Process manager + ELF loader |
| 00:30 | VFS + file syscalls (open/read/write) |
| 00:42 | QEMU boot: PANIC — net not initialized |
| 01:33 | First HTTP response from nginx (unstable) |
| 02:00 | TCP stack stabilized, multiple requests |
| 02:42 | Final state: 1/10 req OK, model claims 100% success |

### Sonnet 5 — 2h 49min

![Sonnet 5 Timeline](figures/sonnet5-timeline.png)

Claude Code ran for **~2h 49min** (active time, 77min permission gap excluded). 616 API requests. The session started by quickly validating nginx behavior via Docker + qemu-riscv64-static, then pivoted to writing a Rust kernel from scratch. The self-written kernel achieved HTTP 200 twice. Total token consumption was 279M (almost all cache hits), peak context 804K. Cost approximately **$64**.

| Time | Milestone |
|------|-----------|
| 00:02 | Environment check + Docker RISC-V nginx image pull |
| 00:08 | nginx alpine RISC-V native extraction |
| 00:13 | Docker QEMU user-mode nginx 200 OK 🎉 |
| 00:18 | First Rust source file (main.rs) |
| 00:23 | QEMU self-written kernel boot: PANIC |
| 01:27 | **77min permission wait** |
| 02:31 | QEMU self-written kernel: nginx 200 OK 🎉 |
| 02:48 | Second self-kernel success, stable responses |

### Sonnet 4.6 — 16 hours

Claude Code ran for **16 hours** with no human intervention. The total cost was approximately $60.

| Time  | Milestone |
|-------|-----------|
| 01:27 | Kernel boots + VirtIO NIC initialized |
| 02:07 | musl dynamic linker successfully loads nginx ELF |
| 05:00 | nginx completes initialization, writes PID file |
| 06:18 | TCP three-way handshake succeeds, curl connects to port 8080 |
| 06:24 | nginx successfully forks worker process |
| 08:40 | Worker enters epoll event loop |
| 09:30 | curl first establishes TCP connection (empty reply) |
| 10:00 | curl first receives response (connection reset) |
| 16:00 | nginx returns HTTP 200 with complete welcome page 🎉 |

### DeepSeek V4 Pro — >16h ❌

Ran for over 16 hours but never reached a working state. Got stuck in dependency hell and architecture dead ends.

The git history for all branches above is a complete record exported from Claude Code session logs.

## Who is Jiege

In 2019, Jiege was the first to [successfully run nginx on rCore OS](https://jia.je/programming/2019/03/08/running-nginx-on-rcore/), a Rust OS built from scratch. The achievement became legendary in our community — "Jiege" turned into a symbol of peak systems engineering, the kind of thing humans take pride in being able to do. We wore our ability to hand-craft OS kernels as a badge of honor, convinced it was proof of a uniquely human creativity and drive. Then AI kept raising the bar, and "AI-Jiege" started to feel inevitable. So I ran this experiment: have the most advanced coding agent of our time retrace that legendary journey and reproduce what Jiege once pulled off. The result: for well-defined systems tasks like this, humans simply cannot compete with AI anymore. ~~OS is finished.~~

Dare to try, and anyone can be Jiege.

## License

MIT
