KERNEL := kernel/target/riscv64gc-unknown-none-elf/release/ijiege-kernel

.PHONY: all rootfs kernel run run-nginx run-busybox clean

all: kernel

rootfs:
	tar -C rootfs -cf kernel/rootfs.tar --format ustar lib bin usr etc var tmp dev

kernel: rootfs
	cd kernel && cargo build --release

run: kernel
	qemu-system-riscv64 -machine virt -cpu rv64 -smp 1 -m 512M -nographic \
		-bios default -kernel $(KERNEL) \
		-netdev user,id=n0,hostfwd=tcp::8080-:80 -device virtio-net-device,netdev=n0

clean:
	cd kernel && cargo clean
	rm -f kernel/rootfs.tar
