//! Kernel-wide constants describing the memory layout on the QEMU `virt` machine.

pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 12;

/// Physical RAM starts here on QEMU virt (fixed by hardware/OpenSBI convention).
pub const PHYS_MEM_BASE: usize = 0x8000_0000;
/// Total RAM size; must match the `-m` flag passed to QEMU.
pub const MEMORY_SIZE: usize = 256 * 1024 * 1024;
pub const MEMORY_END: usize = PHYS_MEM_BASE + MEMORY_SIZE;

/// Trampoline page: mapped at the same virtual address in every address space
/// (kernel and every user process) so trap entry/exit code stays valid across
/// a `satp` switch.
pub const TRAMPOLINE: usize = usize::MAX - PAGE_SIZE + 1;
/// Per-task trap context, mapped one page below the trampoline in user space.
pub const TRAP_CONTEXT: usize = TRAMPOLINE - PAGE_SIZE;

pub const KERNEL_HEAP_SIZE: usize = 16 * 1024 * 1024;

/// Kernel stack size for each task; kernel stacks are laid out with a guard
/// page between them below `TRAMPOLINE`.
pub const KERNEL_STACK_SIZE: usize = 64 * 1024;
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Base of the bump allocator used for `mmap` regions with no
/// caller-specified address, and for placing the dynamic linker/interpreter
/// image. Comfortably inside SV39's low canonical half (below 2^38) and far
/// from ELF load addresses and the user stack/heap.
pub const MMAP_BASE: usize = 0x0000_0020_0000_0000;

/// Fixed load bias for a PIE main executable (`ET_DYN`, no ASLR).
pub const ELF_BASE: usize = 0x0001_0000;
/// Fixed load bias for the dynamic linker/interpreter image, well clear of
/// `ELF_BASE` and `MMAP_BASE`.
pub const INTERP_BASE: usize = 0x0000_0010_0000_0000;

/// virtio-mmio transport slots and the PLIC, as discovered from the QEMU
/// virt machine's device tree (8 virtio-mmio slots at 0x1000 stride, PLIC
/// spanning 0xc000000..0xc600000).
pub const MMIO: &[(usize, usize)] = &[(0x1000_1000, 0x1000 * 8), (0x0c00_0000, 0x60_0000)];
