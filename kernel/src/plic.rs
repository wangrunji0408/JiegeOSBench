//! PLIC (Platform-Level Interrupt Controller) driver for QEMU virt.

use core::arch::asm;

const PLIC_BASE: usize = 0x0C00_0000;
const PLIC_PRIORITY: usize = PLIC_BASE + 0x0000_0000;
const PLIC_ENABLE_S: usize = PLIC_BASE + 0x0020_80; // context 1 (S-mode hart0)
const PLIC_THRESHOLD_S: usize = PLIC_BASE + 0x2010_00; // context 1 threshold
const PLIC_CLAIM_S: usize = PLIC_BASE + 0x2010_04; // context 1 claim/complete

pub fn init() {
    // threshold 0 (all priorities)
    unsafe {
        (PLIC_THRESHOLD_S as *mut u32).write_volatile(0);
    }
}

pub fn enable(irq: u32) {
    unsafe {
        let word = (PLIC_ENABLE_S + (irq as usize / 32) * 4) as *mut u32;
        let v = word.read_volatile() | (1 << (irq % 32));
        word.write_volatile(v);
        // priority > 0
        let prio = (PLIC_PRIORITY + irq as usize * 4) as *mut u32;
        prio.write_volatile(1);
    }
}

pub fn disable(irq: u32) {
    unsafe {
        let word = (PLIC_ENABLE_S + (irq as usize / 32) * 4) as *mut u32;
        let v = word.read_volatile() & !(1 << (irq % 32));
        word.write_volatile(v);
    }
}

/// Returns the highest pending IRQ for S-mode context, or 0.
pub fn claim() -> u32 {
    unsafe { (PLIC_CLAIM_S as *const u32).read_volatile() }
}

pub fn complete(irq: u32) {
    unsafe {
        (PLIC_CLAIM_S as *mut u32).write_volatile(irq);
    }
}

pub fn enable_sie() {
    unsafe {
        // sie.SEIE = 1<<9, sie.STIE = 1<<5
        asm!("csrs sie, {}", in(reg) (1 << 9) | (1 << 5), options(nostack));
        // sstatus.SIE = 1<<1
        asm!("csrs sstatus, {}", in(reg) (1 << 1), options(nostack));
    }
}
