TARGET  := riscv64gc-unknown-none-elf
KERNEL  := target/$(TARGET)/release/jiege-kernel
ROOTFS  := build/rootfs.tar
TOOLCHAIN := +nightly

.PHONY: all build rootfs run debug clean gdb objdump

all: build

# The rootfs is embedded with include_bytes!, so it must exist before the build.
$(ROOTFS): scripts/build-rootfs.sh
	./scripts/build-rootfs.sh

rootfs: $(ROOTFS)

build: $(ROOTFS)
	cargo $(TOOLCHAIN) build --release

run: build
	./scripts/run.sh

# Wait for a debugger on port 1234.
debug: build
	./scripts/run.sh -s -S

gdb:
	riscv64-elf-gdb $(KERNEL) -ex 'target remote :1234'

objdump: build
	$$(find ~/.rustup -name llvm-objdump | head -1) -d --source $(KERNEL) | less

clean:
	cargo clean
	rm -rf build
