//! Kernel console: 16550 UART on the QEMU virt machine, with an input ring buffer
//! fed by the UART RX interrupt.

use crate::sync::SpinLock;
use core::fmt::{self, Write};

pub const UART_BASE: usize = 0x1000_0000;
const RBR: usize = 0; // receive buffer / transmit holding
const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const MCR: usize = 4;
const LSR: usize = 5;

const RX_BUF_SIZE: usize = 4096;

struct Console {
    rx: [u8; RX_BUF_SIZE],
    rx_head: usize,
    rx_tail: usize,
    tx_lock: bool,
}

unsafe impl Send for Console {}

static CONSOLE: SpinLock<Console> = SpinLock::new(Console {
    rx: [0; RX_BUF_SIZE],
    rx_head: 0,
    rx_tail: 0,
    tx_lock: false,
});

/// Set once the kernel page table is active (devices move to the high half).
static mut MMU_ON: bool = false;

pub fn mmu_enabled() {
    unsafe {
        MMU_ON = true;
    }
}

#[inline(always)]
fn reg(off: usize) -> *mut u8 {
    if unsafe { MMU_ON } {
        crate::mm::address::dev_addr(UART_BASE + off) as *mut u8
    } else {
        (UART_BASE + off) as *mut u8
    }
}

pub fn init() {
    unsafe {
        // disable interrupts while configuring
        reg(IER).write_volatile(0x00);
        reg(LCR).write_volatile(0x80); // DLAB
        reg(0).write_volatile(0x03); // 38.4k divisor low
        reg(1).write_volatile(0x00);
        reg(LCR).write_volatile(0x03); // 8N1
        reg(FCR).write_volatile(0x07); // enable + clear FIFOs
        reg(MCR).write_volatile(0x03); // DTR | RTS
        reg(IER).write_volatile(0x01); // RX interrupt enable
    }
}

pub fn enable_rx_interrupt() {
    unsafe {
        reg(IER).write_volatile(0x01);
    }
}

pub fn disable_rx_interrupt() {
    unsafe {
        reg(IER).write_volatile(0x00);
    }
}

fn put_byte(b: u8) {
    unsafe {
        while reg(LSR).read_volatile() & 0x20 == 0 {}
        reg(RBR).write_volatile(b);
    }
}

pub fn write_str_raw(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            put_byte(b'\r');
        }
        put_byte(b);
    }
}

pub fn put_byte_raw(b: u8) {
    put_byte(b);
}

/// Called from the trap handler when the UART raises an interrupt.
pub fn handle_rx_interrupt() {
    loop {
        let lsr = unsafe { reg(LSR).read_volatile() };
        if lsr & 0x01 == 0 {
            break;
        }
        let b = unsafe { reg(RBR).read_volatile() };
        let mut c = CONSOLE.lock();
        let head = c.rx_head;
        let next = (head + 1) % RX_BUF_SIZE;
        if next != c.rx_tail {
            c.rx[head] = b;
            c.rx_head = next;
        }
    }
}

/// Non-blocking read of one byte from the console input buffer.
pub fn try_getchar() -> Option<u8> {
    let mut c = CONSOLE.lock();
    if c.rx_head == c.rx_tail {
        None
    } else {
        let b = c.rx[c.rx_tail];
        c.rx_tail = (c.rx_tail + 1) % RX_BUF_SIZE;
        Some(b)
    }
}

pub fn has_input() -> bool {
    let c = CONSOLE.lock();
    c.rx_head != c.rx_tail
}

struct ConsoleWriter;

impl Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut c = CONSOLE.lock();
        if c.tx_lock {
            return Ok(());
        }
        c.tx_lock = true;
        drop(c);
        write_str_raw(s);
        CONSOLE.lock().tx_lock = false;
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    let _ = ConsoleWriter.write_fmt(args);
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
