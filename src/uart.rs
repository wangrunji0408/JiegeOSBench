use core::fmt::{self, Write};

const UART_BASE: usize = 0x1000_0000;
const RBR: usize = 0;
const THR: usize = 0;
const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const LSR: usize = 5;

fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

pub fn init() {
    unsafe {
        reg(IER).write_volatile(0);
        reg(LCR).write_volatile(0x03);
        reg(FCR).write_volatile(0x07);
    }
}

pub fn putc(c: u8) {
    unsafe {
        while reg(LSR).read_volatile() & 0x20 == 0 {}
        reg(THR).write_volatile(c);
    }
}

pub fn getc() -> Option<u8> {
    unsafe {
        if reg(LSR).read_volatile() & 0x01 != 0 {
            Some(reg(RBR).read_volatile())
        } else {
            None
        }
    }
}

pub struct UartWriter;

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                putc(b'\r');
            }
            putc(b);
        }
        Ok(())
    }
}
