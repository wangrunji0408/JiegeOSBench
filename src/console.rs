use core::fmt::{self, Write};

const UART_BASE: usize = 0x1000_0000;

struct Uart;

impl Uart {
    fn put(&self, byte: u8) {
        unsafe {
            while core::ptr::read_volatile((UART_BASE + 5) as *const u8) & 0x20 == 0 {}
            core::ptr::write_volatile(UART_BASE as *mut u8, byte);
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.put(b'\r');
            }
            self.put(byte);
        }
        Ok(())
    }
}

static mut UART: Uart = Uart;

pub fn init() {
    unsafe {
        core::ptr::write_volatile((UART_BASE + 1) as *mut u8, 0);
        core::ptr::write_volatile((UART_BASE + 3) as *mut u8, 0x03);
        core::ptr::write_volatile((UART_BASE + 2) as *mut u8, 0x01);
    }
}

pub fn _print(args: fmt::Arguments) {
    unsafe {
        (&raw mut UART)
            .as_mut()
            .unwrap_unchecked()
            .write_fmt(args)
            .ok();
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}
