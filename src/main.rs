#![no_std]
#![no_main]
#![allow(static_mut_refs)]
#![allow(unsafe_op_in_unsafe_fn)]

mod arch;
mod console;
mod elf;
mod net;
mod syscall;
mod vfs;

core::arch::global_asm!(include_str!("boot.S"));
core::arch::global_asm!(include_str!("trap.S"));

#[unsafe(no_mangle)]
pub extern "C" fn rust_entry() -> ! {
    clear_bss();
    console::write_str("\nLuna RISC-V kernel\n");
    console::write_str("booted on QEMU virt in supervisor mode\n");
    arch::init();
    syscall::init();
    let _ = net::init();
    elf::start_nginx();
}

unsafe extern "C" {
    static mut __bss_start: u8;
    static mut __bss_end: u8;
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

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    console::write_str("\nKERNEL PANIC: ");
    console::write_panic(info);
    console::write_str("\n");
    loop {
        core::hint::spin_loop();
    }
}
