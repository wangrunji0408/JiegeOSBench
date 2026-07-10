/// 内核起始物理地址（OpenSBI之后）
pub const KERNEL_PHYS_BASE: usize = 0x80200000;

/// 内核使用直接映射（物理地址=虚拟地址），所以偏移为0
pub const KERNEL_OFFSET: usize = 0;

/// 内存总大小 (256MB for QEMU)
pub const MEMORY_SIZE: usize = 0x10000000; // 256MB

/// 物理内存结束
pub const MEMORY_END: usize = 0x80000000 + MEMORY_SIZE;

/// 页大小
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

/// 用户栈大小
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024; // 8MB

/// 内核栈大小
pub const KERNEL_STACK_SIZE: usize = 512 * 1024; // 512KB

/// 最大进程数
pub const MAX_TASKS: usize = 256;

/// 最大文件描述符数
pub const MAX_FDS: usize = 1024;

/// 时钟频率 (QEMU virt)
pub const CLOCK_FREQ: usize = 10_000_000; // 10MHz

/// 时间片 (ms)
pub const TIME_SLICE_MS: usize = 5;

/// virtio-net设备在virt机器中的地址范围
pub const VIRTIO_BASE: usize = 0x10001000;
pub const VIRTIO_SIZE: usize = 0x1000;
pub const VIRTIO_COUNT: usize = 8;

/// UART基地址
pub const UART_BASE: usize = 0x10000000;

/// PLIC基地址
pub const PLIC_BASE: usize = 0x0c000000;

/// 用户PIE基地址
pub const PIE_BASE: usize = 0x40000000;

/// ld.so基地址
pub const INTERP_BASE: usize = 0x50000000;
