//! UART (NS16550) console driver, MMIO at QEMU virt UART0.

use crate::sync::SpinLock;
use core::fmt::{self, Write};

const UART0: usize = 0x1000_0000;

const RHR: usize = 0; // receive holding register (read)
const THR: usize = 0; // transmit holding register (write)
const IER: usize = 1;
const LSR: usize = 5;

const LSR_TX_EMPTY: u8 = 0x20;
const LSR_RX_READY: u8 = 0x01;

static CONSOLE: SpinLock<Console> = SpinLock::new(Console);

struct Console;

unsafe fn uart_read(reg: usize) -> u8 {
    core::ptr::read_volatile((UART0 + reg) as *const u8)
}

unsafe fn uart_write(reg: usize, val: u8) {
    core::ptr::write_volatile((UART0 + reg) as *mut u8, val)
}

impl Console {
    fn putc(&self, c: u8) {
        unsafe {
            // wait until transmit holding register is empty
            while uart_read(LSR) & LSR_TX_EMPTY == 0 {}
            uart_write(THR, c);
        }
    }

    fn getc(&self) -> Option<u8> {
        unsafe {
            if uart_read(LSR) & LSR_RX_READY != 0 {
                Some(uart_read(RHR))
            } else {
                None
            }
        }
    }
}

pub fn init() {
    unsafe {
        // disable interrupts (we poll)
        uart_write(IER, 0x00);
    }
}

pub fn putchar(c: u8) {
    let mut con = CONSOLE.lock();
    con.putc(c);
}

pub fn getchar() -> Option<u8> {
    let con = CONSOLE.lock();
    con.getc()
}

pub fn print_fmt(args: fmt::Arguments) {
    let mut con = CONSOLE.lock();
    let _ = con.write_fmt(args);
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::print_fmt(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::print_fmt(format_args!("{}\n", format_args!($($arg)*))) };
}
