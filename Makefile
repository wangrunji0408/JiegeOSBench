KERNEL := target/riscv64gc-unknown-none-elf/release/jiege-kernel
QEMU := qemu-system-riscv64

.PHONY: build run debug test clean

build:
	cargo build --release

run: build
	$(QEMU) -machine virt -global virtio-mmio.force-legacy=false -bios default -kernel $(KERNEL) -m 256M -smp 1 \
		-netdev user,id=net0,hostfwd=tcp::8080-:80 \
		-device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56 \
		-nographic -monitor none -no-reboot

test:
	./scripts/test-http.sh

debug: build
	$(QEMU) -machine virt -bios default -kernel $(KERNEL) -m 256M -smp 1 \
		-nographic -monitor none -no-reboot -S -s

clean:
	cargo clean
