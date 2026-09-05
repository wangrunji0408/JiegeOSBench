//! Kernel-wide constants.

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

/// Physical RAM (QEMU virt).
pub const RAM_START: usize = 0x8000_0000;
pub const RAM_SIZE: usize = 512 * 1024 * 1024;
pub const RAM_END: usize = RAM_START + RAM_SIZE;

/// Address where QEMU's generic loader places the rootfs cpio archive.
pub const ROOTFS_ADDR: usize = 0x8800_0000;

/// Timer frequency of QEMU virt (rdtime ticks per second).
pub const TIMEBASE_FREQ: u64 = 10_000_000;
/// Scheduler tick interval.
pub const TICK_HZ: u64 = 100;

/// Kernel stack size for each task.
pub const KSTACK_SIZE: usize = 128 * 1024;

/// User address space layout.
pub const USER_MAX: usize = 0x40_0000_0000; // Sv39 user limit
pub const USER_STACK_TOP: usize = 0x3F_FFFF_F000;
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;
pub const MMAP_BASE: usize = 0x20_0000_0000;
pub const PIE_BASE: usize = 0x10_0000_0000;
pub const INTERP_BASE: usize = 0x18_0000_0000;
/// Page holding the signal-return trampoline (mapped read/exec in every process).
pub const SIGRET_TRAMPOLINE: usize = 0x3F_FFF0_0000;

/// Verbose kernel logging.
pub const KLOG: bool = true;
/// Trace every syscall (very noisy).
pub const STRACE: bool = false;

/// Guest network configuration (QEMU user networking defaults).
pub const IP_ADDR: [u8; 4] = [10, 0, 2, 15];
pub const IP_PREFIX: u8 = 24;
pub const GATEWAY: [u8; 4] = [10, 0, 2, 2];
