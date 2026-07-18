//! 内核常量配置

/// 物理内存起始
pub const MEMORY_START: usize = 0x8000_0000;
/// 物理内存大小（QEMU -m 256M）
pub const MEMORY_SIZE: usize = 256 * 1024 * 1024;
pub const MEMORY_END: usize = MEMORY_START + MEMORY_SIZE;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SIZE_BITS: usize = 12;

/// 内核堆大小（位于 .bss）
pub const KERNEL_HEAP_SIZE: usize = 64 * 1024 * 1024;

/// trampoline 虚拟地址（所有地址空间共享同一 VA）
pub const TRAMPOLINE: usize = 0x3f_ffff_f000;
/// TrapContext 所在页的虚拟地址（每个用户地址空间映射各自内核栈顶页）
pub const TRAP_CONTEXT: usize = TRAMPOLINE - PAGE_SIZE;

/// 用户栈大小
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;
/// 用户栈顶（含一页 guard）
pub const USER_STACK_TOP: usize = 0x3f_0000_0000;

/// 内核栈大小
pub const KERNEL_STACK_SIZE: usize = 16 * 1024;

/// mmap 区域起始（向上增长）
pub const MMAP_BASE: usize = 0x20_0000_0000;

/// 时钟频率（QEMU virt timebase = 10MHz）
pub const CLOCK_FREQ: u64 = 10_000_000;
/// 时钟中断间隔
pub const TICKS_PER_SEC: u64 = 100;

/// 文件描述符上限
pub const FD_LIMIT: usize = 1024;
