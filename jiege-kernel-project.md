---
name: jiege-kernel-project
description: The iJiege-opus5 repo is a from-scratch riscv64 Rust kernel ("智能杰哥") whose goal is running the unmodified official nginx binary and serving HTTP from QEMU — goal achieved 2026-07-27
metadata:
  type: project
---

`/Users/wangrunji/Codes/iJiege-opus5` is a from-scratch RISC-V (rv64gc) kernel in
Rust called 智能杰哥 / `jiege-kernel`. The goal, stated by the user on 2026-07-27
and **achieved the same day**, was to run the *official, unmodified* Alpine
riscv64 nginx binary (`nginx-1.30.4-r2`, a dynamically linked PIE against musl)
and serve HTTP reachable from the host.

Working state: `make && make run`, then `curl http://localhost:8080/` returns 200
from `Server: nginx/1.30.4`. Measured ~1185 req/s keep-alive, 50/50 concurrent
connections, byte-exact bodies.

**Why:** the user asked for a complete, self-designed implementation with no
questions asked and no modification of the target binary — so every capability
nginx exercises (ELF dynamic loading, fork/exec, signals, futexes, epoll, BSD
sockets) had to be real rather than stubbed.

**How to apply:** the four bugs that cost the most debugging time are documented
in `README.md` under "Bugs worth remembering", each with a comment at the code
site. See [[riscv-linux-abi-gotchas]] for the ones that generalize beyond this
repo. Build needs Rust nightly; the rootfs is embedded via `include_bytes!` so
`scripts/build-rootfs.sh` must run before `cargo build`.
