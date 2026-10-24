KERNEL := target/riscv64gc-unknown-none-elf/debug/jiege-kernel
QEMU := qemu-system-riscv64
QEMU_FLAGS := -machine virt -m 512M -smp 1 -bios default -nographic -no-reboot -global virtio-mmio.force-legacy=false -netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 -device virtio-net-device,netdev=n0,mac=52:54:00:12:34:56

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
