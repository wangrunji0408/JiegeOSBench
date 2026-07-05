//! NS16550A 串口 (UART) 驱动。QEMU virt 平台的串口寄存器位于 0x1000_0000。

use core::ptr::write_volatile;
use core::ptr::read_volatile;

pub const UART0: usize = 0x1000_0000;

// NS16550A 寄存器偏移（DLAB=0 时）
const RBR: usize = 0; // 接收缓冲 / 发送保持
const IER: usize = 1; // 中断使能
const FCR: usize = 2; // FIFO 控制
const LCR: usize = 3; // 线路控制
const MCR: usize = 4; // Modem 控制
const LSR: usize = 5; // 线路状态

const LSR_THRE: u8 = 0x20; // 发送保持寄存器空
const LSR_DR: u8 = 0x01;   // 数据就绪

/// 初始化 UART：8N1, 38400 baud (QEMU 不关心波特率，但仍按真实硬件流程)
pub fn init() {
    unsafe {
        // 关中断
        write_reg(IER, 0x00);
        // 打开 DLAB 设置波特率分频
        write_reg(LCR, 0x80);
        write_reg(0, 0x03); // 分频低字节
        write_reg(1, 0x00); // 分频高字节
        // 8 bit, 无校验, 1 停止位, 关 DLAB
        write_reg(LCR, 0x03);
        // 使能 FIFO, 清空, 14 字节阈值
        write_reg(FCR, 0xC7);
        // RTS/DSR, OUT2 (允许中断路由)
        write_reg(MCR, 0x0B);
    }
}

#[inline]
unsafe fn write_reg(off: usize, val: u8) {
    write_volatile((UART0 + off) as *mut u8, val);
}

#[inline]
unsafe fn read_reg(off: usize) -> u8 {
    read_volatile((UART0 + off) as *const u8)
}

/// 阻塞发送一个字节
pub fn putc(c: u8) {
    unsafe {
        while read_reg(LSR) & LSR_THRE == 0 {
            // spin
        }
        write_reg(RBR, c);
    }
}

/// 读取一个字节，无数据返回 None
pub fn getc() -> Option<u8> {
    unsafe {
        if read_reg(LSR) & LSR_DR != 0 {
            Some(read_reg(RBR))
        } else {
            None
        }
    }
}
