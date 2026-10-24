#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod console;
mod drivers;
mod elf;
mod fd;
mod fs;
mod heap;
mod memory;
mod process;
mod sbi;
mod syscall;

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
extern "C" fn rust_main(hart_id: usize, dtb: usize) -> ! {
    console::init();
    println!("jiege-kernel: Rust RISC-V kernel booted");
    println!("hart={hart_id} dtb={dtb:#x}");
    memory::init();
    memory::activate();
    arch::init();
    arch::enable_interrupts();
    println!("Sv39 active; timer and external interrupts enabled");
    loop {
        arch::wait_for_interrupt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::disable_interrupts();
    println!("kernel panic: {info}");
    loop {
        arch::wait_for_interrupt();
    }
}
