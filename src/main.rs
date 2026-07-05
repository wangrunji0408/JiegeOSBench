//! JiegeOS — a RISC-V OS kernel in Rust that runs the official nginx binary.
#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

#[macro_use]
mod console;
mod fs;
mod loader;
mod mm;
mod net;
mod sbi;
mod syscall;
mod task;
mod time;
mod trap;

use core::arch::global_asm;
use core::panic::PanicInfo;

pub const DEBUG_SYSCALLS: bool = false;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    la sp, {stack}
    li t0, {stack_size}
    add sp, sp, t0
    j rust_main
"#,
    stack = sym BOOT_STACK,
    stack_size = const BOOT_STACK_SIZE,
);

const BOOT_STACK_SIZE: usize = 64 * 1024;

#[repr(C, align(16))]
struct Stack([u8; BOOT_STACK_SIZE]);

#[no_mangle]
static mut BOOT_STACK: Stack = Stack([0; BOOT_STACK_SIZE]);

#[no_mangle]
extern "C" fn rust_main(hartid: usize, _dtb: usize) -> ! {
    // .bss is zero: QEMU RAM starts zeroed and -kernel only copies filesz
    println!("\n[jiege-os] booting on hart {}", hartid);
    mm::init();
    trap::init();
    fs::init();

    // build kernel page table and enable paging
    let kernel_pt = mm::paging::PageTable::new();
    unsafe { kernel_pt.activate() };
    println!("[mm] paging enabled (Sv39)");

    net::init();

    // create the nginx task
    let mut t = task::Task::new();
    unsafe { t.pt.activate() };
    let info = loader::exec(
        &mut t,
        "/usr/sbin/nginx",
        &["/usr/sbin/nginx"],
        &["PATH=/usr/sbin:/usr/bin:/bin", "HOME=/root"],
    );
    t.tf.sepc = info.entry;
    *t.tf.sp() = info.sp;
    task::set_current(t);

    println!("[jiege-os] entering user mode\n");
    let t = task::current();
    loop {
        let (scause, stval) = trap::run_user(&mut t.tf);
        match scause {
            trap::SCAUSE_ECALL_U => {
                t.tf.sepc += 4;
                let nr = t.tf.syscall_nr();
                let args = t.tf.syscall_args();
                let ret = syscall::dispatch(nr, args);
                if t.exit_code.is_some() {
                    break;
                }
                t.tf.set_ret(ret);
            }
            trap::SCAUSE_IFAULT | trap::SCAUSE_LFAULT | trap::SCAUSE_SFAULT => {
                println!(
                    "[fatal] user page fault: scause={} stval={:#x} sepc={:#x}",
                    scause, stval, t.tf.sepc
                );
                break;
            }
            _ => {
                println!(
                    "[fatal] unexpected trap: scause={:#x} stval={:#x} sepc={:#x}",
                    scause, stval, t.tf.sepc
                );
                break;
            }
        }
    }
    println!("[jiege-os] user task finished, shutting down");
    sbi::shutdown();
}

fn clear_bss() {
    extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of_mut!(__bss_start);
        let end = core::ptr::addr_of_mut!(__bss_end);
        let len = end as usize - start as usize;
        // The boot stack is at the start of .bss and in use — skip it.
        let stack_size = 1024 * 64;
        core::ptr::write_bytes(start.add(stack_size), 0, len - stack_size);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n[kernel PANIC] {}", info);
    sbi::shutdown()
}
