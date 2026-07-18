#![no_std]
#![no_main]
#![allow(dead_code)]
#![allow(unused_imports)]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

mod config;
mod console;
mod fs;
mod mm;
mod sbi;
mod syscall;
mod task;
mod trap;
mod lang_items {
    use super::*;

    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        crate::println!("[kernel] panicked: {}", info);
        crate::sbi::shutdown(true);
    }
}

global_asm!(include_str!("entry.asm"));

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    clear_bss();
    println!("[kernel] hello from riscv64 kernel!");
    trap::init();
    mm::init();
    fs::init();
    println!("[kernel] paging enabled, kernel heap + frame allocator + rootfs online");

    println!("[kernel] loading initproc (/lib/ld-musl-riscv64.so.1 direct test)...");
    let nginx_data = {
        let file = fs::open_file("/lib/ld-musl-riscv64.so.1", 0).expect("interp binary missing from rootfs");
        let size = file.size();
        let mut buf = alloc::vec![0u8; size];
        let mut off = 0;
        while off < size {
            let n = file.read_at(off, &mut buf[off..]);
            if n == 0 {
                break;
            }
            off += n;
        }
        buf
    };
    let args = alloc::vec![
        alloc::string::String::from("/lib/ld-musl-riscv64.so.1"),
        alloc::string::String::from("/usr/sbin/nginx"),
        alloc::string::String::from("-g"),
        alloc::string::String::from("daemon off;"),
    ];
    let envs = alloc::vec![alloc::string::String::from("PATH=/usr/sbin:/usr/bin:/sbin:/bin")];
    task::add_initproc(&nginx_data, &args, &envs);
    task::run_tasks();
}

fn clear_bss() {
    unsafe extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as *const () as usize;
        let end = ebss as *const () as usize;
        core::slice::from_raw_parts_mut(start as *mut u8, end - start).fill(0);
    }
}
