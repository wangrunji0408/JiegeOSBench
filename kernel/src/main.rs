#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

#[macro_use]
pub mod csr;
pub mod console;
pub mod fdt;
pub mod fs;
pub mod lang;
pub mod mm;
pub mod net;
pub mod plic;
pub mod sbi;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod time;
pub mod trap;

use core::arch::global_asm;

global_asm!(include_str!("entry.S"));
global_asm!(include_str!("trap.S"));

extern "C" {
    fn ekernel();
}

/// The boot stack lives in `.bss.stack`; `sbss` (linker) is its top.
#[no_mangle]
#[link_section = ".bss.stack"]
static mut BOOT_STACK: [u8; 64 * 1024] = [0; 64 * 1024];

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    console::init();
    crate::println!();
    crate::println!("=====================================================");
    crate::println!(" iJiege kernel - a RISC-V Linux-ABI compatible kernel");
    crate::println!(" boot hart={} dtb={:#x}", hartid, dtb);
    crate::println!("=====================================================");

    let fdt = fdt::Fdt::new(dtb).expect("no valid device tree");
    let (ram_start, ram_size) = fdt.memory().expect("no /memory node");
    let initrd = fdt.initrd();
    let bootargs = fdt.bootargs().unwrap_or("");
    crate::println!(
        "[boot] ram {:#x}..{:#x} initrd {:x?}",
        ram_start,
        ram_start + ram_size,
        initrd
    );
    crate::println!("[boot] cmdline: {}", bootargs);

    let kernel_end = ekernel as usize;
    let mut reserved = alloc::vec![(ram_start, kernel_end)];
    if let Some((s, e)) = initrd {
        reserved.push((s, e));
    }
    mm::frame::init(ram_start, ram_start + ram_size, &reserved);
    mm::heap::init();

    // Build and install the kernel page table.
    let root = mm::frame::alloc_frame().expect("no frame for kernel page table");
    mm::page_table::init_kernel(root, ram_start, ram_start + ram_size, initrd);
    csr::set_satp((8usize << 60) | (root >> 12));
    mm::set_memory_map(mm::MemoryMap {
        ram_start,
        ram_end: ram_start + ram_size,
        initrd_start: initrd.map(|x| x.0).unwrap_or(0),
        initrd_end: initrd.map(|x| x.1).unwrap_or(0),
    });

    trap::init();
    plic::init();
    plic::register(10, |_| console::handle_rx_interrupt());
    console::enable_rx_interrupt();
    time::init();

    task::init();
    fs::init();
    net::init();

    // Launch the init process (or drop into the idle loop if there is none).
    task::start_init();

    task::start()
}
