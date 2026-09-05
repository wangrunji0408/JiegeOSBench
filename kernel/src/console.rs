//! 16550 UART console (QEMU virt: 0x1000_0000, IRQ 10)
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::sync::SpinLock;
use crate::task::wait::WaitQueue;

pub const UART_BASE: usize = 0x1000_0000;
pub const UART_IRQ: usize = 10;

const RBR: usize = 0;
const THR: usize = 0;
const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const LSR: usize = 5;

#[inline]
fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

pub fn init() {
    unsafe {
        // disable interrupts
        reg(IER).write_volatile(0x00);
        // 8 bits, no parity
        reg(LCR).write_volatile(0x03);
        // enable FIFO, clear
        reg(FCR).write_volatile(0x07);
        // enable receive interrupt
        reg(IER).write_volatile(0x01);
    }
}

pub fn putchar(c: u8) {
    unsafe {
        while reg(LSR).read_volatile() & 0x20 == 0 {}
        reg(THR).write_volatile(c);
    }
}

fn try_getchar() -> Option<u8> {
    unsafe {
        if reg(LSR).read_volatile() & 0x01 != 0 {
            Some(reg(RBR).read_volatile())
        } else {
            None
        }
    }
}

struct InputBuf {
    buf: [u8; 1024],
    head: usize,
    tail: usize,
}

static INPUT: SpinLock<InputBuf> = SpinLock::new(InputBuf { buf: [0; 1024], head: 0, tail: 0 });
pub static INPUT_WQ: WaitQueue = WaitQueue::new();
static ECHO: AtomicBool = AtomicBool::new(true);

/// Called from the UART interrupt handler.
pub fn handle_irq() {
    let mut got = false;
    while let Some(c) = try_getchar() {
        let mut ib = INPUT.lock();
        let next = (ib.tail + 1) % ib.buf.len();
        if next != ib.head {
            let t = ib.tail;
            ib.buf[t] = c;
            ib.tail = next;
        }
        got = true;
    }
    if got {
        INPUT_WQ.wake_all();
    }
}

pub fn input_available() -> bool {
    let ib = INPUT.lock();
    ib.head != ib.tail
}

pub fn read_input(dst: &mut [u8]) -> usize {
    let mut ib = INPUT.lock();
    let mut n = 0;
    while n < dst.len() && ib.head != ib.tail {
        let h = ib.head;
        let mut c = ib.buf[h];
        ib.head = (h + 1) % ib.buf.len();
        if c == b'\r' {
            c = b'\n';
        }
        if ECHO.load(Ordering::Relaxed) {
            if c == 0x7f || c == 8 {
                putchar(8);
                putchar(b' ');
                putchar(8);
            } else {
                putchar(c);
            }
        }
        dst[n] = c;
        n += 1;
    }
    n
}

pub fn set_echo(on: bool) {
    ECHO.store(on, Ordering::Relaxed);
}

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            putchar(b);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    Stdout.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::print(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Kernel log with a prefix; controlled by the global verbosity.
#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => {
        if $crate::config::KLOG {
            $crate::console::print(format_args!("[kernel] {}\n", format_args!($($arg)*)))
        }
    };
}
