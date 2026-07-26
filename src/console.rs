//! Kernel console: `print!` / `println!` backed by the SBI debug console.

use crate::sbi;
use core::fmt::{self, Write};
use spin::Mutex;

struct Stdout;

/// Serializes console output so concurrent hart/task prints don't interleave
/// mid-line.
static CONSOLE_LOCK: Mutex<()> = Mutex::new(());

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        // DBCN takes a physical address; the kernel is identity mapped so the
        // slice pointer works as-is. Fall back to putchar if unsupported.
        if !sbi::console_write(bytes) {
            for &b in bytes {
                sbi::console_putchar(b);
            }
        }
        Ok(())
    }
}

pub fn print_fmt(args: fmt::Arguments) {
    let _guard = CONSOLE_LOCK.lock();
    let _ = Stdout.write_fmt(args);
}

/// Print without taking the lock — used from the panic path, where the lock may
/// already be held by the faulting context.
pub fn print_fmt_nolock(args: fmt::Arguments) {
    let _ = Stdout.write_fmt(args);
}

/// Write raw bytes (used by the tty device for user-space writes).
pub fn write_bytes(bytes: &[u8]) {
    let _guard = CONSOLE_LOCK.lock();
    if !sbi::console_write(bytes) {
        for &b in bytes {
            sbi::console_putchar(b);
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::print_fmt(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::print_fmt(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Log a line at "info" level with a green tag.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::println!("\x1b[32m[jiege]\x1b[0m {}", format_args!($($arg)*))
    };
}

/// Log a warning.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::println!("\x1b[33m[warn]\x1b[0m {}", format_args!($($arg)*))
    };
}

/// Verbose tracing, compiled in but gated on a runtime flag.
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::console::trace_enabled() {
            $crate::println!("\x1b[90m[trace]\x1b[0m {}", format_args!($($arg)*))
        }
    };
}

use core::sync::atomic::{AtomicBool, Ordering};
static TRACE: AtomicBool = AtomicBool::new(false);

pub fn trace_enabled() -> bool {
    TRACE.load(Ordering::Relaxed)
}

pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}
