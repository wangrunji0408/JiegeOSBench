KERNEL := target/riscv64gc-unknown-none-elf/debug/jiege-kernel
QEMU := qemu-system-riscv64
QEMU_FLAGS := -machine virt -m 512M -smp 1 -bios default -nographic -no-reboot

.PHONY: build check run clean

build:
	cargo build

check:
	cargo check
	cargo fmt --check

run: build
	$(QEMU) $(QEMU_FLAGS) -kernel $(KERNEL)

clean:
	cargo clean
