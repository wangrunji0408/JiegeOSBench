# iJiegeOS

[中文版](./README-CN.md)

A Rust OS kernel autonomously implemented by Claude Code (Sonnet 4.6) — just barely capable of running a real Linux nginx web server on QEMU.

## Prompt (Translation)

```
You are the AI-Jiege. Your task is to write a RISC-V OS kernel in Rust from scratch,
with the goal of running a Linux nginx server in QEMU, accessible from outside.
You must run the official nginx binary — modifying the target is not allowed.
Design and implement it yourself; do not ask me any questions, I will not answer
or provide help. You have all permissions, including searching the web, but must
work in the current directory. Keep working until the goal is achieved.
```

⏵⏵ bypass permissions on

## Timeline

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

The git history is a complete record exported from the Claude Code session logs.

## Demo

```
$ ./run.sh
$ curl http://127.0.0.1:8080/
```

## Background

In 2019, Jiege was the first to [successfully run nginx on rCore OS](https://jia.je/programming/2019/03/08/running-nginx-on-rcore/), a Rust OS built from scratch. The achievement became legendary in our community — "Jiege" turned into a symbol of peak systems engineering, the kind of thing humans take pride in being able to do. We wore our ability to hand-craft OS kernels as a badge of honor, convinced it was proof of a uniquely human creativity and drive. Then AI kept raising the bar, and "AI-Jiege" started to feel inevitable. So I ran this experiment: have the most advanced coding agent of our time retrace that legendary journey and reproduce what Jiege once pulled off. The result: for well-defined systems tasks like this, humans simply cannot compete with AI anymore. ~~OS is finished.~~

Dare to try, and anyone can be Jiege.

## License

MIT
