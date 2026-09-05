//! A small RISC-V kernel with a Linux-compatible system call interface,
//! sufficient to run unmodified Linux/musl binaries such as nginx.
#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

#[macro_use]
pub mod console;
pub mod abi;
pub mod config;
pub mod drivers;
pub mod fs;
pub mod mm;
pub mod net;
pub mod sbi;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod time;
pub mod trap;

use alloc::vec::Vec;
use core::arch::global_asm;

global_asm!(include_str!("entry.S"));

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, _dtb: usize) -> ! {
    console::init();
    println!();
    println!("=== jiege-os (riscv64) booting on hart {} ===", hartid);
    trap::init();
    time::init();
    // Heap: everything except kernel image and the rootfs archive. We must know
    // where the archive ends, so parse it first with a temporary tiny heap.
    let rootfs_end = probe_rootfs_end(config::ROOTFS_ADDR);
    mm::init(rootfs_end);
    task::init();
    drivers::plic::init();
    drivers::plic::enable(console::UART_IRQ);
    fs::init(config::ROOTFS_ADDR);
    net::init();

    let (used, total) = mm::heap::stats();
    klog!("heap: {} KiB used of {} MiB", used / 1024, total / 1024 / 1024);

    // Launch init.
    let init_path = "/init";
    let argv: Vec<Vec<u8>> = alloc::vec![init_path.as_bytes().to_vec()];
    let envp: Vec<Vec<u8>> = alloc::vec![
        b"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_vec(),
        b"HOME=/root".to_vec(),
        b"TERM=vt100".to_vec(),
    ];
    task::process::spawn_init(init_path, argv, envp);
    klog!("starting init");
    task::sched::idle_loop()
}

/// Walk the cpio archive headers to find its end without allocating.
fn probe_rootfs_end(base: usize) -> usize {
    let mut off = 0usize;
    loop {
        let hdr = unsafe { core::slice::from_raw_parts((base + off) as *const u8, 110) };
        if &hdr[0..6] != b"070701" {
            return base + off;
        }
        let hex = |b: &[u8]| -> usize {
            let mut v = 0usize;
            for &c in b {
                v = v * 16
                    + match c {
                        b'0'..=b'9' => (c - b'0') as usize,
                        b'a'..=b'f' => (c - b'a' + 10) as usize,
                        b'A'..=b'F' => (c - b'A' + 10) as usize,
                        _ => 0,
                    };
            }
            v
        };
        let filesize = hex(&hdr[54..62]);
        let namesize = hex(&hdr[94..102]);
        let name = unsafe { core::slice::from_raw_parts((base + off + 110) as *const u8, namesize.saturating_sub(1)) };
        let data_start = (off + 110 + namesize + 3) & !3;
        off = (data_start + filesize + 3) & !3;
        if name == b"TRAILER!!!" {
            return base + off;
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    trap::csr::disable_interrupts();
    println!();
    println!("!!! KERNEL PANIC !!!");
    if let Some(loc) = info.location() {
        println!("at {}:{}:{}", loc.file(), loc.line(), loc.column());
    }
    println!("{}", info.message());
    if let Some(t) = task::try_current() {
        println!("current task: pid {} ({})", t.pid, t.name());
        let tf = t.tf();
        println!("user sepc={:#x} sp={:#x} ra={:#x}", tf.sepc, tf.sp(), tf.x[1]);
    }
    // Print a simple backtrace using frame pointers.
    let mut fp: usize;
    unsafe { core::arch::asm!("mv {}, s0", out(reg) fp) };
    println!("backtrace:");
    for _ in 0..24 {
        if fp < config::RAM_START || fp >= config::RAM_END || fp % 8 != 0 {
            break;
        }
        let ra = unsafe { *((fp - 8) as *const usize) };
        let next = unsafe { *((fp - 16) as *const usize) };
        println!("  {:#x}", ra);
        if next <= fp {
            break;
        }
        fp = next;
    }
    sbi::shutdown()
}
