#![no_std]
#![no_main]

use core::fmt::{self, Write};

core::arch::global_asm!(include_str!("boot.S"));

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

#[unsafe(no_mangle)]
pub extern "C" fn rust_entry() -> ! {
    clear_bss();
    let mut uart = Uart;
    let _ = writeln!(uart, "\nLuna RISC-V kernel");
    let _ = writeln!(uart, "booted on QEMU virt in supervisor mode");
    let _ = writeln!(uart, "kernel end = {:#x}", kernel_end());
    loop { core::hint::spin_loop(); }
}

unsafe extern "C" {
    static mut __bss_start: u8;
    static mut __bss_end: u8;
    static __kernel_end: u8;
}

fn clear_bss() {
    unsafe {
        let mut p = &raw mut __bss_start;
        let end = &raw mut __bss_end;
        while p < end {
            core::ptr::write_volatile(p, 0);
            p = p.add(1);
        }
    }
}

fn kernel_end() -> usize {
    unsafe { &__kernel_end as *const u8 as usize }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut uart = Uart;
    let _ = writeln!(uart, "\nKERNEL PANIC: {}", info);
    loop { core::hint::spin_loop(); }
}
