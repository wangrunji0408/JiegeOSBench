//! Per-task trap context, saved/restored across the user<->kernel boundary
//! by the trampoline (`trampoline.S`).

use riscv::register::sstatus;

/// Field offsets (in 8-byte units) must match `trampoline.S` exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    /// satp token of the kernel address space, loaded by `__alltraps`
    /// before jumping into `trap_handler`.
    pub kernel_satp: usize,
    /// Per-task kernel stack pointer to switch to on trap entry.
    pub kernel_sp: usize,
    /// Kernel VA of the `trap_handler` function.
    pub trap_handler: usize,
}

impl TrapContext {
    pub fn set_sp(&mut self, sp: usize) {
        self.x[2] = sp;
    }

    pub fn app_init_context(
        entry: usize,
        sp: usize,
        kernel_satp: usize,
        kernel_sp: usize,
        trap_handler: usize,
    ) -> Self {
        const SPIE: usize = 1 << 5;
        const FS_INITIAL: usize = 1 << 13;
        let sstatus_bits = SPIE | FS_INITIAL;
        let mut cx = Self {
            x: [0; 32],
            sstatus: sstatus_bits,
            sepc: entry,
            kernel_satp,
            kernel_sp,
            trap_handler,
        };
        cx.set_sp(sp);
        cx
    }
}
