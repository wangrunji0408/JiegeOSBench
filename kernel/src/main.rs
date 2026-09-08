#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

#[macro_use]
pub mod csr;
pub mod console;
pub mod errno;
pub mod fdt;
pub mod fs;
pub mod lang;
pub mod mm;
pub mod net;
pub mod plic;
pub mod sbi;
pub mod socket;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod time;
pub mod trap;

use core::arch::global_asm;

global_asm!(include_str!("entry.S"));
global_asm!(include_str!("trap.S"));
global_asm!(include_str!("fp.S"));

/// Kernel command line passed by the bootloader.
pub struct BootArgs(crate::sync::SpinLock<alloc::string::String>);
impl BootArgs {
    pub const fn new() -> Self {
        Self(crate::sync::SpinLock::new(alloc::string::String::new()))
    }
    pub fn store(&self, s: &str) {
        *self.0.lock() = alloc::string::String::from(s);
    }
    pub fn load(&self) -> alloc::string::String {
        self.0.lock().clone()
    }
}
pub static BOOTARGS: BootArgs = BootArgs::new();

extern "C" {
    fn ekernel();
}

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
    let mut reserved = [(0usize, 0usize); 3];
    reserved[0] = (ram_start, kernel_end);
    let mut nreserved = 1;
    if let Some((s, e)) = initrd {
        reserved[nreserved] = (s, e);
        nreserved += 1;
    }
    mm::frame::init(ram_start, ram_start + ram_size, &reserved[..nreserved]);
    mm::heap::init();
    BOOTARGS.store(bootargs);
    if bootargs.contains("traceall") {
        syscall::TRACE_ALL.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    // Build and install the kernel page table.
    let root = mm::frame::alloc_frame().expect("no frame for kernel page table");
    mm::page_table::init_kernel(root, ram_start, ram_start + ram_size, initrd);
    crate::println!("[boot] kernel page table at {:#x}", root);
    csr::set_satp((8usize << 60) | (root >> 12));
    console::mmu_enabled();
    task::set_kernel_root(root);
    crate::println!("[boot] satp installed");
    mm::set_memory_map(mm::MemoryMap {
        ram_start,
        ram_end: ram_start + ram_size,
        initrd_start: initrd.map(|x| x.0).unwrap_or(0),
        initrd_end: initrd.map(|x| x.1).unwrap_or(0),
    });

    trap::init();
    crate::println!("[boot] traps ready");
    plic::init();
    plic::register(10, |_| console::handle_rx_interrupt());
    console::enable_rx_interrupt();
    time::init();
    crate::println!("[boot] timer armed, realtime {} ms", time::realtime_ms());

    task::init();
    crate::println!("[boot] scheduler ready");
    fs::init();
    net::init();

    // Launch the init process (or drop into the idle loop if there is none).
    task::start_init();

    task::start()
}
