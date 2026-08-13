//! Trap handling: supervisor interrupts and exceptions, and the context-switch
//! machinery shared between kernel and user mode.

use core::arch::global_asm;

/// Saved CPU context on trap. Layout must match the assembly below.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapContext {
    /// General purpose registers x0..x31 (x0 is unused but present for simplicity).
    pub x: [usize; 32],
    /// Saved sstatus CSR (offset 32*8).
    pub sstatus: usize,
    /// Saved sepc CSR (offset 33*8).
    pub sepc: usize,
}

impl TrapContext {
    pub const fn zero() -> Self {
        Self {
            x: [0; 32],
            sstatus: 0,
            sepc: 0,
        }
    }

    /// Create a context that will start user mode at `entry` with stack `sp`.
    pub fn new_user(entry: usize, sp: usize) -> Self {
        let mut cx = Self::zero();
        cx.sepc = entry;
        cx.x[2] = sp; // user stack pointer
        // SPIE=1 (bit5) so sret enables interrupts in user mode; SPP=0 (user).
        // SUM=1 (bit18) so the kernel (S-mode) may directly access user pages
        // during syscall handling. FS=3 (bits 13-14) enables floating point.
        cx.sstatus = (1 << 5) | (1 << 18) | (3 << 13);
        cx
    }

    pub fn set_ret(&mut self, val: usize) {
        self.x[10] = val; // a0
    }
}

global_asm!(
    r#"
    .section .text.trap
    .globl __alltraps
    .globl __trap_return
    .globl trap_entry
    .align 2

trap_entry:
    j __alltraps

# On entry:
#   - from user: sscratch holds kernel stack top, sp holds user sp
#   - from kernel: sscratch holds 0, sp holds kernel sp
__alltraps:
    csrrw sp, sscratch, sp       # sp <-> sscratch
    bnez sp, 1f                  # sp != 0 => came from user
    csrr sp, sscratch            # came from kernel: restore kernel sp
1:
    addi sp, sp, -34*8           # reserve TrapContext on kernel stack
    sd x1, 1*8(sp)
    sd x3, 3*8(sp)
    sd x4, 4*8(sp)
    sd x5, 5*8(sp)
    sd x6, 6*8(sp)
    sd x7, 7*8(sp)
    sd x8, 8*8(sp)
    sd x9, 9*8(sp)
    sd x10, 10*8(sp)
    sd x11, 11*8(sp)
    sd x12, 12*8(sp)
    sd x13, 13*8(sp)
    sd x14, 14*8(sp)
    sd x15, 15*8(sp)
    sd x16, 16*8(sp)
    sd x17, 17*8(sp)
    sd x18, 18*8(sp)
    sd x19, 19*8(sp)
    sd x20, 20*8(sp)
    sd x21, 21*8(sp)
    sd x22, 22*8(sp)
    sd x23, 23*8(sp)
    sd x24, 24*8(sp)
    sd x25, 25*8(sp)
    sd x26, 26*8(sp)
    sd x27, 27*8(sp)
    sd x28, 28*8(sp)
    sd x29, 29*8(sp)
    sd x30, 30*8(sp)
    sd x31, 31*8(sp)
    # save original sp (from sscratch swap above)
    csrr t0, sscratch
    sd t0, 2*8(sp)
    # save sstatus and sepc
    csrr t0, sstatus
    sd t0, 32*8(sp)
    csrr t0, sepc
    sd t0, 33*8(sp)
    # entering kernel: sscratch = 0
    csrw sscratch, x0
    # call trap_handler(&mut cx) -> &mut cx
    mv a0, sp
    call trap_handler

# Return to (possibly different) context pointed by a0.
__trap_return:
    # a0 = TrapContext*
    ld t0, 32*8(a0)           # saved sstatus
    andi t1, t0, 0x100        # SPP: 0 => user, 1 => kernel
    csrw sstatus, t0
    ld t0, 33*8(a0)           # saved sepc
    csrw sepc, t0
    # prepare sscratch for the next trap
    beqz t1, 1f
    csrw sscratch, x0         # returning to kernel
    j 2f
1:
    addi t2, a0, 34*8         # kernel stack top
    csrw sscratch, t2         # returning to user
2:
    mv sp, a0                 # use sp as the context base
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    ld x5, 5*8(sp)
    ld x6, 6*8(sp)
    ld x7, 7*8(sp)
    ld x8, 8*8(sp)
    ld x9, 9*8(sp)
    ld x10, 10*8(sp)
    ld x11, 11*8(sp)
    ld x12, 12*8(sp)
    ld x13, 13*8(sp)
    ld x14, 14*8(sp)
    ld x15, 15*8(sp)
    ld x16, 16*8(sp)
    ld x17, 17*8(sp)
    ld x18, 18*8(sp)
    ld x19, 19*8(sp)
    ld x20, 20*8(sp)
    ld x21, 21*8(sp)
    ld x22, 22*8(sp)
    ld x23, 23*8(sp)
    ld x24, 24*8(sp)
    ld x25, 25*8(sp)
    ld x26, 26*8(sp)
    ld x27, 27*8(sp)
    ld x28, 28*8(sp)
    ld x29, 29*8(sp)
    ld x30, 30*8(sp)
    ld x31, 31*8(sp)
    ld sp, 2*8(sp)            # restore sp last
    sret
"#
);

/// Trap cause: interrupt vs exception (scause high bit).
const INTERRUPT: usize = 1 << 63;

/// Supervisor timer interrupt.
const TIMER_IRQ: usize = 5;

pub fn init() {
    unsafe {
        core::arch::asm!(
            "csrw stvec, {tvec}",
            tvec = in(reg) trap_entry as *const () as usize,
        );
        // enable supervisor timer interrupt (STIE)
        let sie: usize = 1 << 5;
        core::arch::asm!("csrs sie, {}", in(reg) sie);
        // enable interrupts in sstatus (SIE)
        let sie: usize = 1 << 1;
        core::arch::asm!("csrs sstatus, {}", in(reg) sie);
    }
}

extern "C" {
    fn trap_entry();
}

/// Entry point for the assembly trampoline.
#[no_mangle]
extern "C" fn trap_handler(cx: *mut TrapContext) -> *mut TrapContext {
    unsafe {
        let scause: usize;
        core::arch::asm!("csrr {}, scause", out(reg) scause);

        if scause & INTERRUPT != 0 {
            // Interrupt
            match scause & !INTERRUPT {
                TIMER_IRQ => {
                    crate::timer_tick();
                }
                other => {
                    crate::println!("[trap] unhandled interrupt: scause={:#x}", other);
                }
            }
        } else {
            // Exception
            match scause {
                // Environment call from U-mode (syscall)
                8 => {
                    let cx = &mut *cx;
                    let ret = crate::syscall::dispatch(cx);
                    cx.x[10] = ret as usize; // a0
                    cx.sepc += 4;
                }
                _ => {
                    crate::println!(
                        "[trap] exception: scause={:#x}, sepc={:#x}, stval={:#x}",
                        scause, (*cx).sepc, stval()
                    );
                    crate::sbi::shutdown();
                }
            }
        }
        cx
    }
}

fn stval() -> usize {
    let v: usize;
    unsafe { core::arch::asm!("csrr {}, stval", out(reg) v) };
    v
}

/// Switch to a context (used by the scheduler). Never returns to the caller;
/// instead begins running `cx` as if returning from a trap.
pub fn switch_to(cx: *mut TrapContext) -> ! {
    unsafe {
        core::arch::asm!(
            "mv a0, {cx}",
            "j __trap_return",
            cx = in(reg) cx,
            options(noreturn)
        );
    }
}
