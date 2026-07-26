//! Raw CSR access helpers.

use core::arch::asm;

macro_rules! read_csr {
    ($mod_name:ident, $csr:literal) => {
        pub mod $mod_name {
            #[inline]
            pub fn read() -> usize {
                let v: usize;
                unsafe { core::arch::asm!(concat!("csrr {}, ", $csr), out(reg) v, options(nostack)) };
                v
            }
        }
    };
}

read_csr!(scause, "scause");
read_csr!(stval, "stval");
read_csr!(sepc, "sepc");

pub mod sstatus {
    use super::*;

    const SIE: usize = 1 << 1;
    const SUM: usize = 1 << 18;

    #[inline]
    pub fn read() -> usize {
        let v: usize;
        unsafe { asm!("csrr {}, sstatus", out(reg) v, options(nostack)) };
        v
    }

    /// # Safety
    /// Enabling interrupts in the wrong place breaks critical sections.
    #[inline]
    pub unsafe fn set_sie() {
        asm!("csrs sstatus, {}", in(reg) SIE, options(nostack));
    }

    /// Clear SIE, returning whether it had been set.
    ///
    /// # Safety
    /// Callers must re-enable interrupts to avoid stalling the scheduler.
    #[inline]
    pub unsafe fn clear_sie() -> bool {
        let old: usize;
        asm!("csrrc {}, sstatus, {}", out(reg) old, in(reg) SIE, options(nostack));
        old & SIE != 0
    }

    /// # Safety
    /// Must only be called during kernel init.
    #[inline]
    pub unsafe fn set_sum() {
        asm!("csrs sstatus, {}", in(reg) SUM, options(nostack));
    }
}

pub mod sie {
    use super::*;

    const SSIE: usize = 1 << 1;
    const STIE: usize = 1 << 5;
    const SEIE: usize = 1 << 9;

    /// Enable timer, external and software interrupts.
    ///
    /// # Safety
    /// Only valid once the trap vector and drivers are installed.
    #[inline]
    pub unsafe fn enable_all() {
        asm!("csrs sie, {}", in(reg) SSIE | STIE | SEIE, options(nostack));
    }
}

pub mod sip {
    use super::*;

    const SSIP: usize = 1 << 1;

    /// # Safety
    /// Called from the interrupt handler only.
    #[inline]
    pub unsafe fn clear_ssoft() {
        asm!("csrc sip, {}", in(reg) SSIP, options(nostack));
    }
}

/// Read the cycle counter.
#[inline]
pub fn cycle() -> u64 {
    let v: u64;
    unsafe { asm!("rdcycle {}", out(reg) v, options(nostack)) };
    v
}

/// Read the `time` CSR (the machine timer, at a fixed 10 MHz in QEMU virt).
#[inline]
pub fn time() -> u64 {
    let v: u64;
    unsafe { asm!("rdtime {}", out(reg) v, options(nostack)) };
    v
}

/// Set the thread pointer, which we use to hold the current hart's ID.
#[inline]
pub fn hart_id() -> usize {
    let v: usize;
    unsafe { asm!("mv {}, tp", out(reg) v, options(nostack)) };
    v
}

/// Wait for an interrupt.
#[inline]
pub fn wfi() {
    unsafe { asm!("wfi", options(nostack)) };
}
