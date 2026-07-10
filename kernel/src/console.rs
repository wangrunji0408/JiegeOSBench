use core::fmt::{self, Write};
use spin::Mutex;

pub struct Console;

static CONSOLE: Mutex<Console> = Mutex::new(Console);

// 通过SBI输出字符（更可靠）
pub fn putchar(c: u8) {
    if c == b'\n' {
        sbi_putchar(b'\r');
    }
    sbi_putchar(c);
}

fn sbi_putchar(c: u8) {
    // SBI legacy: sbi_console_putchar (EID=1)
    unsafe {
        core::arch::asm!(
            "li a7, 1",
            "mv a0, {0}",
            "ecall",
            in(reg) c as usize,
            out("a0") _,
            options(nomem, nostack)
        );
    }
}

pub fn init() {
    // SBI控制台无需初始化
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            putchar(c);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        $crate::console::_print(format_args!($($arg)*));
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn _print(args: fmt::Arguments) {
    CONSOLE.lock().write_fmt(args).unwrap();
}
