use core::fmt::{self, Write};

const UART0: usize = 0x1000_0000;
const UART_THR: usize = 0;
const UART_LSR: usize = 5;
const LSR_TX_IDLE: u8 = 1 << 5;

pub struct Console;

impl Console {
    pub fn put_byte(byte: u8) {
        unsafe {
            while (core::ptr::read_volatile((UART0 + UART_LSR) as *const u8) & LSR_TX_IDLE) == 0 {}
            core::ptr::write_volatile((UART0 + UART_THR) as *mut u8, byte);
        }
    }
}

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                Self::put_byte(b'\r');
            }
            Self::put_byte(byte);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    Console.write_fmt(args).unwrap();
}

pub fn put_byte(byte: u8) {
    Console::put_byte(byte);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
