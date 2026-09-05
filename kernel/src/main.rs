#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![allow(dead_code)]
#![allow(static_mut_refs)]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

#[macro_use]
mod uart;
mod config;
mod elf;
mod file;
mod frame;
mod fs;
mod heap;
mod memory;
mod net;
mod page_table;
mod sbi;
mod syscall;
mod task;
mod time;
mod trap;

use alloc::sync::Arc;
use file::{FileDesc, FileKind};
use spin::Mutex;

global_asm!(
    ".section .text.entry",
    ".globl _start",
    "_start:",
    "   la sp, boot_stack_top",
    "   call rust_main",
    "1: wfi",
    "   j 1b",
    ".section .bss.stack",
    ".globl boot_stack_lower",
    "boot_stack_lower:",
    "   .space 4096 * 64",
    ".globl boot_stack_top",
    "boot_stack_top:",
);

fn clear_bss() {
    extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as usize;
        let end = ebss as usize;
        let mut p = start;
        while p + 8 <= end {
            (p as *mut u64).write_volatile(0);
            p += 8;
        }
        while p < end {
            (p as *mut u8).write_volatile(0);
            p += 1;
        }
    }
}

static NGINX_ELF: &[u8] = include_bytes!("../embed/nginx");
static NGINX_CONF: &[u8] = include_bytes!("../embed/nginx.conf");
static INDEX_HTML: &[u8] = include_bytes!("../embed/index.html");

fn setup_fs() {
    fs::init();
    fs::mkdir_p("/nginx/conf");
    fs::mkdir_p("/nginx/logs");
    fs::mkdir_p("/nginx/html");
    fs::mkdir_p("/tmp");
    fs::mkdir_p("/nginx/client_body_temp");
    fs::mkdir_p("/nginx/proxy_temp");
    fs::mkdir_p("/nginx/fastcgi_temp");
    fs::mkdir_p("/nginx/uwsgi_temp");
    fs::mkdir_p("/nginx/scgi_temp");
    fs::write_file("/nginx/conf/nginx.conf", NGINX_CONF);
    fs::write_file("/nginx/html/index.html", INDEX_HTML);
    fs::write_file("/nginx/html/50x.html", INDEX_HTML);
}

fn console_fd(readable: bool, writable: bool) -> Arc<Mutex<FileDesc>> {
    Arc::new(Mutex::new(FileDesc {
        kind: FileKind::Console,
        offset: 0,
        flags: 0,
        readable,
        writable,
    }))
}

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, _dtb: usize) -> ! {
    clear_bss();
    uart::init();
    println!();
    println!("[kernel] boot hart {}", hartid);
    memory::init();
    println!("[kernel] paging on, free frames: {}", frame::free_count());
    trap::init();
    setup_fs();
    println!("[kernel] ramfs ready");
    net::init();

    // Build the user address space.
    let pt = page_table::PageTable::new();
    memory::map_kernel(&pt);
    unsafe {
        memory::activate(pt.satp());
    }
    let t = task::Task::new(pt);
    task::install(t);

    // stdin/stdout/stderr
    {
        let t = task::current();
        t.fds.set(0, console_fd(true, false), false);
        t.fds.set(1, console_fd(false, true), false);
        t.fds.set(2, console_fd(false, true), false);
    }

    let argv = ["nginx", "-c", "/nginx/conf/nginx.conf"];
    let envp = ["TZ=UTC"];
    let (entry, sp) = elf::load(task::current(), NGINX_ELF, &argv, &envp);
    println!("[kernel] loaded nginx: entry={:#x} sp={:#x}", entry, sp);
    println!("[kernel] entering user mode");
    task::current().enter_user(entry, sp);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[kernel] PANIC: {}", info);
    sbi::shutdown();
}
