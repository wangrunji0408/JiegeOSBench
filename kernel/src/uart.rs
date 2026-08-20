//! NS16550A UART (QEMU virt @ 0x1000_0000)，寄存器间隔 1 字节

const UART_BASE: usize = 0x1000_0000;

#[inline]
fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

const RBR: usize = 0; // 读
const THR: usize = 0; // 写
const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const MCR: usize = 4;
const LSR: usize = 5;
const LSR_THRE: u8 = 0x20; // THR 空

pub fn early_init() {
    // 关中断、FIFO、8N1、DTR|RTS|OUT2 —— QEMU 下主要走个过场
    unsafe {
        reg(IER).write_volatile(0x00);
        reg(LCR).write_volatile(0x03);
        reg(FCR).write_volatile(0x01);
        reg(MCR).write_volatile(0x03);
    }
}

pub fn put_byte(b: u8) {
    // 等待 THR 空
    while unsafe { reg(LSR).read_volatile() } & LSR_THRE == 0 {}
    unsafe { reg(THR).write_volatile(b) };
}

pub fn write_bytes(mut s: &[u8]) {
    while !s.is_empty() {
        let b = s[0];
        if b == b'\n' {
            put_byte(b'\r');
        }
        put_byte(b);
        s = &s[1..];
    }
}

pub fn write_str(s: &str) {
    write_bytes(s.as_bytes());
}

/// 读取一个字节（无数据返回 None）
pub fn get_byte() -> Option<u8> {
    unsafe {
        if reg(LSR).read_volatile() & 0x01 != 0 {
            Some(reg(RBR).read_volatile())
        } else {
            None
        }
    }
}
