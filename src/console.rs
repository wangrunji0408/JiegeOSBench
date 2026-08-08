use core::fmt::{self, Write};

const UART0: usize = 0x1000_0000;

struct Uart;

impl Uart {
    fn put(&self, byte: u8) {
        unsafe {
            while core::ptr::read_volatile((UART0 + 5) as *const u8) & 0x20 == 0 {}
            core::ptr::write_volatile(UART0 as *mut u8, byte);
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' { self.put(b'\r'); }
            self.put(byte);
        }
        Ok(())
    }
}

pub fn write_str(s: &str) { let _ = Uart.write_str(s); }
pub fn write_bytes(s: &[u8]) { for &b in s { Uart.put(b); } }
pub fn write_fmt(args: &fmt::Arguments<'_>) { let _ = Uart.write_fmt(*args); }
pub fn write_panic(info: &core::panic::PanicInfo<'_>) { let _ = Uart.write_fmt(format_args!("{}", info)); }

pub fn write_hex(mut v: usize) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    buf[0] = b'0'; buf[1] = b'x';
    for i in 0..16 { buf[17-i] = digits[v & 0xf]; v >>= 4; }
    write_bytes(&buf);
}

pub fn write_hex_byte(v: u8) {
    let digits = b"0123456789abcdef";
    write_bytes(&[digits[(v >> 4) as usize], digits[(v & 0xf) as usize]]);
}

pub fn write_dec(mut v: usize) {
    let mut buf = [0u8; 20];
    let mut p = buf.len();
    if v == 0 { write_bytes(b"0"); return; }
    while v != 0 { p -= 1; buf[p] = b'0' + (v % 10) as u8; v /= 10; }
    write_bytes(&buf[p..]);
}
