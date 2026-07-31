#![no_std]
#![no_main]
#![feature(panic_info_message)]

extern crate alloc;

mod console;
mod dtb;
mod elf;
mod epoll;
mod fs;
mod lang;
mod mm;
mod net;
mod plic;
mod sbi;
mod signal;
mod syscall;
mod task;
mod timer;
mod timer_wheel;
mod trap;
mod uart;
mod virtio;

use core::arch::global_asm;

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("trap.S"));

pub const INITRAMFS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../initramfs/initramfs.cpio"
));

extern "C" {
    static _kernel_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
    fn trap_entry();
}

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    uart::init();
    crate::kprintln!("==================================================");
    crate::kprintln!(" JiegeOS - a from-scratch RISC-V kernel in Rust");
    crate::kprintln!("==================================================");
    crate::kprintln!("[boot] hart={} dtb={:#x}", hartid, dtb);

    // 1. device tree
    let info = dtb::parse(dtb);
    let ram_end = info.ram_base + info.ram_size;
    crate::kprintln!(
        "[boot] RAM {:#x}..{:#x} ({} MiB), timebase={} Hz",
        info.ram_base,
        ram_end,
        info.ram_size / 1024 / 1024,
        info.timebase
    );
    if let Some(args) = info.bootargs {
        crate::kprintln!("[boot] bootargs: {}", args);
    }

    // 2. memory
    let kernel_end = unsafe { &_kernel_end as *const u8 as usize };
    mm::init(kernel_end, ram_end);
    crate::kprintln!("[mm] kernel image ends at {:#x}", kernel_end);
    mm::paging::write_satp(mm::kernel_pt().root_ppn());
    crate::kprintln!("[mm] paging enabled (Sv39)");

    // 3. timer
    timer::init(info.timebase);

    // 4. filesystem from embedded initramfs
    unsafe {
        fs::FS = Some(fs::Fs { files: alloc::vec::Vec::new() });
    }
    fs::unpack_cpio(INITRAMFS);

    // 5. network
    net::sock_init();
    virtio::init();

    // 6. interrupts
    unsafe {
        core::arch::asm!(
            "la {0}, trap_entry",
            "csrw stvec, {0}",
            out(reg) _,
            options(nostack)
        );
    }
    plic::init();
    plic::enable(1); // virtio-net IRQ
    plic::enable_sie();

    crate::kprintln!("[trap] stvec set, interrupts enabled");

    // 7. tasks
    task::init_tables();
    crate::kprintln!("[init] starting /init ...");
    syscall::process::start_init("/init");
}
