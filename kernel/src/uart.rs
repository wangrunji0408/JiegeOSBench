use core::fmt::{self, Write};

// NS16550A UART on QEMU virt machine.
const UART_BASE: usize = 0x1000_0000;

const RBR: usize = 0; // receive buffer (read)
const THR: usize = 0; // transmit holding (write)
const IER: usize = 1; // interrupt enable
const FCR: usize = 2; // fifo control (write)
const LCR: usize = 3; // line control
const LSR: usize = 5; // line status

const LSR_THRE: u8 = 1 << 5; // transmit holding empty
const LSR_DR: u8 = 1 << 0; // data ready

#[inline]
fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

pub fn init() {
    unsafe {
        // Disable interrupts.
        reg(IER).write_volatile(0x00);
        // Enable DLAB to set baud divisor.
        reg(LCR).write_volatile(0x80);
        reg(0).write_volatile(0x03); // divisor low
        reg(1).write_volatile(0x00); // divisor high
        // 8 bits, no parity, one stop bit; clear DLAB.
        reg(LCR).write_volatile(0x03);
        // Enable FIFO, clear them.
        reg(FCR).write_volatile(0x07);
    }
}

pub fn putchar(c: u8) {
    unsafe {
        while reg(LSR).read_volatile() & LSR_THRE == 0 {}
        reg(THR).write_volatile(c);
    }
}

pub fn getchar() -> Option<u8> {
    unsafe {
        if reg(LSR).read_volatile() & LSR_DR != 0 {
            Some(reg(RBR).read_volatile())
        } else {
            None
        }
    }
}

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                putchar(b'\r');
            }
            putchar(b);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    Stdout.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::uart::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
