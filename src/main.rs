#![no_std]
#![no_main]

mod arch;
mod console;
mod sbi;

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
extern "C" fn rust_main(hart_id: usize, dtb: usize) -> ! {
    console::init();
    println!("jiege-kernel: Rust RISC-V kernel booted");
    println!("hart={hart_id} dtb={dtb:#x}");
    arch::init();
    println!("memory and interrupt initialization pending");
    loop {
        arch::wait_for_interrupt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("kernel panic: {info}");
    loop {
        arch::wait_for_interrupt();
    }
}
