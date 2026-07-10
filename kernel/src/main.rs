#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(naked_functions)]
#![feature(asm_const)]

#[macro_use]
extern crate alloc;

#[macro_use]
mod console;
mod arch;
mod config;
mod drivers;
mod fs;
mod mm;
mod net;
mod syscall;
mod task;
mod timer;
mod utils;

use core::arch::global_asm;

global_asm!(include_str!("arch/riscv64/boot.S"));

#[no_mangle]
pub extern "C" fn kernel_main(hartid: usize, dtb_pa: usize) -> ! {
    console::init();
    println!("[JiegeOS] Booting on hart {}", hartid);
    println!("[JiegeOS] DTB at {:#x}", dtb_pa);

    mm::init();
    println!("[JiegeOS] Memory initialized");

    arch::trap::init();
    println!("[JiegeOS] Trap initialized");

    timer::init();
    println!("[JiegeOS] Timer initialized");

    drivers::init(dtb_pa);
    println!("[JiegeOS] Drivers initialized");

    fs::init();
    println!("[JiegeOS] Filesystem initialized");

    net::init();
    println!("[JiegeOS] Network initialized");

    task::init();
    println!("[JiegeOS] Starting init process...");

    panic!("kernel_main returned!");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\x1b[1;31m[PANIC]\x1b[0m: {}", info.message());
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}
