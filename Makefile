KERNEL := target/riscv64gc-unknown-none-elf/debug/jiege-kernel
QEMU := qemu-system-riscv64
HOST_PORT ?= 8080
URL := http://127.0.0.1:$(HOST_PORT)
QEMU_FLAGS := -machine virt -m 512M -smp 1 -bios default -nographic -no-reboot -global virtio-mmio.force-legacy=false -netdev user,id=n0,hostfwd=tcp:127.0.0.1:$(HOST_PORT)-:80 -device virtio-net-device,netdev=n0,mac=52:54:00:12:34:56

SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
.ONESHELL:

.PHONY: build check rootfs run verify clean

rootfs:
	python3 scripts/fetch_rootfs.py

build: rootfs
	cargo build

check: rootfs
	cargo check
	cargo fmt --check

run: build
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL)

verify: build
	mkdir -p artifacts; \
	: > artifacts/verify-curl.log; \
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL) > artifacts/verify-kernel.log 2>&1 & \
	qemu_pid=$$!; \
	trap 'kill "$$qemu_pid" 2>/dev/null || true; wait "$$qemu_pid" 2>/dev/null || true' EXIT; \
	ready=0; \
	for attempt in {1..40}; do \
		if ! kill -0 "$$qemu_pid" 2>/dev/null; then \
			break; \
		fi; \
		if curl --noproxy '*' --fail --max-time 3 --silent --show-error \
			--dump-header artifacts/verify-index.headers \
			--output artifacts/verify-index.html \
			$(URL)/ >> artifacts/verify-curl.log 2>&1; then \
			ready=1; \
			break; \
		fi; \
		sleep 0.25; \
	done; \
	if [[ $$ready -ne 1 ]]; then \
		python3 -c 'from pathlib import Path; print(Path("artifacts/verify-curl.log").read_text()); print(Path("artifacts/verify-kernel.log").read_text()[-12000:])'; \
		exit 1; \
	fi; \
	curl --noproxy '*' --fail --max-time 6 --silent --show-error \
		--dump-header artifacts/verify-index.headers \
		--output artifacts/verify-index.html $(URL)/ \
		--dump-header artifacts/verify-reuse.headers \
		--output artifacts/verify-reuse.html $(URL)/; \
	status=$$(curl --noproxy '*' --max-time 6 --silent --show-error \
		--dump-header artifacts/verify-404.headers \
		--output artifacts/verify-404.html \
		--write-out '%{http_code}' $(URL)/missing); \
	[[ $$status == 404 ]]; \
	kill "$$qemu_pid" 2>/dev/null || true; \
	wait "$$qemu_pid" 2>/dev/null || true; \
	trap - EXIT; \
	python3 -c 'from pathlib import Path; import hashlib, json; expected = json.loads(Path("artifacts/rootfs-source.json").read_text())["nginx"]["sha256"]; actual = hashlib.sha256(Path("rootfs/usr/sbin/nginx").read_bytes()).hexdigest(); assert actual == expected; first = Path("artifacts/verify-index.html").read_text(); second = Path("artifacts/verify-reuse.html").read_text(); assert "RISCV_NGINX_OK" in first and "RISCV_NGINX_OK" in second; assert b"404 Not Found" in Path("artifacts/verify-404.html").read_bytes(); headers = Path("artifacts/verify-index.headers").read_text(); assert "Server: nginx/1.28.3" in headers; log = Path("artifacts/verify-kernel.log").read_text(); forbidden = ["[emerg]", "[alert]", "[error]", "unsupported syscall", "kernel panic"]; assert not any(item in log for item in forbidden); print("VERIFY_OK: official nginx 1.28.3, repeated HTTP 200 and HTTP 404, no kernel errors")'

clean:
	cargo clean
