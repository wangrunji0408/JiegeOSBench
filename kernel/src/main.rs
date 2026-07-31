#![no_std]
#![no_main]
#![feature(panic_info_message)]
#![feature(alloc_error_handler)]

#[macro_use]
extern crate alloc;

#[global_allocator]
static ALLOC: mm::heap::HeapAllocator = mm::heap::HeapAllocator;

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
    static _kernel_heap_start: u8;
    static __bss_start: u8;
    static __bss_end: u8;
    static boot_stack_top: u8;
    fn trap_entry();
}

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    uart::init();
    // enable FPU (FS=11) for user programs
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) (3u64 << 13), options(nostack));
    }
    // tp/sscratch = boot stack top until the first task starts
    let boot_top = unsafe { &boot_stack_top as *const u8 as usize };
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) boot_top, options(nostack));
        core::arch::asm!("csrw sscratch, {0}", in(reg) boot_top, options(nostack));
    }
    crate::kprintln!("==================================================");
    crate::kprintln!(" JiegeOS - a from-scratch RISC-V kernel in Rust");
    crate::kprintln!("==================================================");
    crate::kprintln!("[boot] hart={} dtb={:#x}", hartid, dtb);

    // 1. kernel heap (linker-reserved) must be ready before any allocation
    let kernel_end = unsafe { &_kernel_end as *const u8 as usize };
    let heap_start = unsafe { &_kernel_heap_start as *const u8 as usize };
    mm::heap::init(heap_start);
    crate::kprintln!("[mm] kernel heap at {:#x} (64 MiB)", heap_start);

    // 2. device tree (needs heap)
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

    // 3. memory: frame allocator + paging
    mm::init(kernel_end, ram_end);
    crate::kprintln!("[mm] kernel image ends at {:#x}", kernel_end);
    mm::paging::write_satp(mm::kernel_pt().root_ppn());
    crate::kprintln!("[mm] paging enabled (Sv39)");

    // 4. timer
    timer::init(info.timebase);

    // 5. filesystem from embedded initramfs
    unsafe {
        fs::FS = Some(fs::Fs { files: alloc::vec::Vec::new() });
    }
    fs::unpack_cpio(INITRAMFS);

    // 6. network
    net::sock_init();
    virtio::init();

    // 7. interrupts
    unsafe {
        core::arch::asm!(
            "la {0}, trap_entry",
            "csrw stvec, {0}",
            out(reg) _,
            options(nostack)
        );
    }
    plic::init();
    plic::enable(virtio::device_irq()); // virtio-net IRQ
    plic::enable_sie();

    crate::kprintln!("[trap] stvec set, interrupts enabled");

    // 8. tasks
    task::init_tables();
    crate::kprintln!("[init] starting /init ...");
    syscall::process::start_init("/init");
}
