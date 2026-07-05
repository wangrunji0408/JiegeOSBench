//! 控制台：把 core::fmt 输出导向 UART，提供 print!/println! 宏。

use core::fmt::{self, Write};
use crate::uart;

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                uart::putc(b'\r');
            }
            uart::putc(b);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    Stdout.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}
