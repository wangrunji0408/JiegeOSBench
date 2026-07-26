//! Trap context: the saved user register state.

/// Saved user state. The field order is fixed by `trap.S`, which indexes this
/// structure with byte offsets — do not reorder.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapContext {
    /// x0..x31. `x[0]` is unused (hardwired zero) but kept so register numbers
    /// index directly.
    pub x: [usize; 32],
    /// Program counter to resume at (`sepc`).
    pub sepc: usize,
    /// Saved `sstatus`.
    pub sstatus: usize,
    /// Kernel stack pointer for this task, restored on trap entry.
    pub kernel_sp: usize,
    /// Floating point registers f0..f31, saved lazily.
    pub f: [u64; 32],
    /// Saved `fcsr`.
    pub fcsr: usize,
}

/// `sstatus` bits.
pub const SSTATUS_SIE: usize = 1 << 1;
pub const SSTATUS_SPIE: usize = 1 << 5;
pub const SSTATUS_SPP: usize = 1 << 8;
pub const SSTATUS_FS: usize = 3 << 13;
pub const SSTATUS_FS_CLEAN: usize = 1 << 13;
pub const SSTATUS_FS_DIRTY: usize = 3 << 13;
pub const SSTATUS_SUM: usize = 1 << 18;

impl TrapContext {
    /// Build the context for entering user space fresh.
    pub fn new_user(entry: usize, sp: usize, kernel_sp: usize) -> Self {
        let mut ctx = Self::default();
        ctx.sepc = entry;
        ctx.x[2] = sp; // sp
        ctx.kernel_sp = kernel_sp;
        // SPP = 0 (return to U-mode), SPIE = 1 (enable interrupts in user mode),
        // SUM = 1 (kernel may access user pages), FS = initial so the FPU is
        // usable without a trap.
        ctx.sstatus = SSTATUS_SPIE | SSTATUS_SUM | SSTATUS_FS_CLEAN;
        ctx
    }

    #[inline]
    pub fn set_return(&mut self, value: usize) {
        self.x[10] = value; // a0
    }

    #[inline]
    pub fn arg(&self, i: usize) -> usize {
        self.x[10 + i] // a0..a5
    }

    #[inline]
    pub fn syscall_number(&self) -> usize {
        self.x[17] // a7
    }

    #[inline]
    pub fn sp(&self) -> usize {
        self.x[2]
    }

    #[inline]
    pub fn set_sp(&mut self, sp: usize) {
        self.x[2] = sp;
    }

    #[inline]
    pub fn set_tls(&mut self, tp: usize) {
        self.x[4] = tp;
    }
}

/// Kernel-side saved state for a context switch between tasks.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TaskContext {
    /// Where to resume execution (`ra`).
    pub ra: usize,
    /// Kernel stack pointer.
    pub sp: usize,
    /// Callee-saved registers s0..s11.
    pub s: [usize; 12],
}

impl TaskContext {
    /// A context that, when switched to, starts running `entry` on `sp`.
    pub fn new(entry: usize, sp: usize) -> Self {
        Self {
            ra: entry,
            sp,
            s: [0; 12],
        }
    }
}
