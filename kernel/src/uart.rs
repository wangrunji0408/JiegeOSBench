//! 16550 UART driver for QEMU virt (MMIO at 0x10000000).

use core::arch::asm;

pub const UART_BASE: usize = 0x1000_0000;

const THR: usize = 0; // transmit holding
const LSR: usize = 5; // line status

#[inline]
fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

pub fn init() {
    // QEMU's 16550 is pre-configured; just drain.
    unsafe {
        while reg(LSR).read_volatile() & 0x20 == 0 {}
    }
}

pub fn putc(c: u8) {
    unsafe {
        while reg(LSR).read_volatile() & 0x20 == 0 {
            asm!("nop", options(nomem, nostack));
        }
        reg(THR).write_volatile(c);
    }
}

pub fn puts(s: &str) {
    for &b in s.as_bytes() {
        putc(b);
    }
}
