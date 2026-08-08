TARGET := riscv64gc-unknown-none-elf
KERNEL := target/$(TARGET)/release/luna

.PHONY: build run clean

build:
	cargo build --release

run: build
	qemu-system-riscv64 -machine virt -nographic -bios default -kernel $(KERNEL)

clean:
	cargo clean

