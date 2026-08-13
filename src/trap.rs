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
        // SPIE=1 (bit5), so sret enables interrupts in user mode; SPP=0 (user)
        cx.sstatus = 1 << 5;
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
    .align 2

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
    ld t0, 32*8(a0)
    andi t1, t0, 0x100          # SPP bit: 0 => user, 1 => kernel
    csrw sstatus, t0
    ld t0, 33*8(a0)
    csrw sepc, t0
    # restore regs
    ld x1, 1*8(a0)
    ld x3, 3*8(a0)
    ld x4, 4*8(a0)
    ld x5, 5*8(a0)
    ld x6, 6*8(a0)
    ld x7, 7*8(a0)
    ld x8, 8*8(a0)
    ld x9, 9*8(a0)
    ld x10, 10*8(a0)
    ld x11, 11*8(a0)
    ld x12, 12*8(a0)
    ld x13, 13*8(a0)
    ld x14, 14*8(a0)
    ld x15, 15*8(a0)
    ld x16, 16*8(a0)
    ld x17, 17*8(a0)
    ld x18, 18*8(a0)
    ld x19, 19*8(a0)
    ld x20, 20*8(a0)
    ld x21, 21*8(a0)
    ld x22, 22*8(a0)
    ld x23, 23*8(a0)
    ld x24, 24*8(a0)
    ld x25, 25*8(a0)
    ld x26, 26*8(a0)
    ld x27, 27*8(a0)
    ld x28, 28*8(a0)
    ld x29, 29*8(a0)
    ld x30, 30*8(a0)
    ld x31, 31*8(a0)
    ld sp, 2*8(a0)
    # set sscratch: kernel sp top if returning to user, else 0
    addi t2, a0, 34*8
    beqz t1, 2f
    csrw sscratch, x0           # to kernel
    sret
2:
    csrw sscratch, t2           # to user
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
            tvec = in(reg) trap_entry as usize,
        );
        // enable supervisor timer interrupt (STIE)
        let sie: usize = 1 << 5;
        core::arch::asm!("csrs sie, {}", in(reg) sie);
        // enable interrupts in sstatus (SIE)
        core::arch::asm!("csrsi sstatus, {}", in(reg) 1 << 1);
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
        let mut sepc: usize;
        core::arch::asm!("csrr {}, scause", out(reg) scause);
        core::arch::asm!("csrr {}, sepc", out(reg) sepc);

        if scause & INTERRUPT != 0 {
            // Interrupt
            match scause & !INTERRUPT {
                TIMER_IRQ => {
                    let cx = &mut *cx;
                    sepc = cx.sepc;
                    crate::timer_tick();
                }
                other => {
                    crate::println!("[trap] unhandled interrupt: scause={:#x}", other);
                }
            }
        } else {
            // Exception
            let cx_ref = &mut *cx;
            sepc = cx_ref.sepc;
            crate::println!(
                "[trap] exception: scause={:#x}, sepc={:#x}, stval={:#x}",
                scause, sepc, stval()
            );
            crate::sbi::shutdown();
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
