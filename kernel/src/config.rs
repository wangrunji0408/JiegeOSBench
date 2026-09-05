// Physical memory layout for QEMU virt machine.
pub const PHYS_START: usize = 0x8000_0000;
// We launch QEMU with -m 1024M; RAM ends here. Keep in sync with run script.
pub const PHYS_END: usize = 0x8000_0000 + 0x4000_0000; // 1 GiB

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

// Kernel heap carved right after the kernel image.
pub const KERNEL_HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

// MMIO regions (QEMU virt).
pub const UART_BASE: usize = 0x1000_0000;
pub const VIRTIO0_BASE: usize = 0x1000_1000;
pub const VIRTIO_STRIDE: usize = 0x1000;
pub const VIRTIO_COUNT: usize = 8;
pub const PLIC_BASE: usize = 0x0c00_0000;
pub const CLINT_BASE: usize = 0x0200_0000;

// User-space virtual layout (all below PHYS_START so it never collides with the
// kernel's identity-mapped RAM gigapage at [0x8000_0000, 0xC000_0000)).
pub const USER_STACK_TOP: usize = 0x4000_0000;
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MiB
pub const MMAP_BASE: usize = 0x2000_0000;

// Timer: QEMU virt runs the mtime clock at 10 MHz.
pub const CLOCK_FREQ: u64 = 10_000_000;
