# 智能杰哥 OS 构建脚本
TARGET := riscv64gc-unknown-none-elf
MODE   := release
BIN    := target/$(TARGET)/$(MODE)/kernel

RUSTC := rustup run nightly-2026-06-12 cargo

.PHONY: all build run debug clean

all: build

build:
	$(RUSTC) build --$(MODE)

run: build
	qemu-system-riscv64 \
		-machine virt \
		-nographic \
		-bios default \
		-serial mon:stdio \
		-netdev user,id=net0,hostfwd=tcp::8080-:80 \
		-device virtio-net-device,netdev=net0 \
		-kernel $(BIN)

clean:
	$(RUSTC) clean
