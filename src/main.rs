//! 智能杰哥 (jiege) — a RISC-V operating system kernel in Rust, built to run
//! unmodified Linux binaries. The target is the official nginx riscv64 build,
//! serving HTTP to the outside world from inside QEMU.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

mod arch;
mod console;
mod drivers;
mod entry;
mod fs;
mod loader;
mod mm;
mod net;
mod sbi;
mod signal;
mod syscall;
mod task;
mod time;
mod trap;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

/// The root filesystem, built by `make rootfs` and embedded here.
static ROOTFS_ARCHIVE: &[u8] = include_bytes!("../build/rootfs.tar");

/// Clear `.bss`, which the linker reserves but the loader does not zero.
fn clear_bss() {
    extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as usize;
        let end = ebss as usize;
        core::ptr::write_bytes(start as *mut u8, 0, end - start);
    }
}

/// Set to true to log every syscall. Handy while bringing new binaries up; the
/// output is voluminous, so it defaults off.
const TRACE_SYSCALLS: bool = option_env!("JIEGE_TRACE").is_some();

#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb: usize) -> ! {
    clear_bss();
    console::set_trace(TRACE_SYSCALLS);

    println!();
    println!("\x1b[1;36m╔══════════════════════════════════════════════╗\x1b[0m");
    println!("\x1b[1;36m║  智能杰哥 · jiege-kernel  (riscv64, Rust)    ║\x1b[0m");
    println!("\x1b[1;36m╚══════════════════════════════════════════════╝\x1b[0m");
    info!("booting on hart {} (dtb at {:#x})", hartid, dtb);

    // Memory first: everything else allocates.
    mm::init();

    // Trap handling, then interrupts.
    trap::init();
    time::init();

    // Devices, then the filesystem, then the network stack.
    drivers::init();
    fs::init(ROOTFS_ARCHIVE);
    net::init();

    unsafe { arch::sie::enable_all() };

    let (used, total) = mm::frame::stats();
    info!(
        "memory: {} MiB free of {} MiB physical, {} MiB kernel heap",
        (total - used) * mm::PAGE_SIZE / 1024 / 1024,
        total * mm::PAGE_SIZE / 1024 / 1024,
        mm::heap::total() / 1024 / 1024,
    );

    // Launch the first user process.
    match start_init() {
        Ok(()) => {}
        Err(e) => {
            println!();
            warn!("failed to start init: errno {}", e.errno());
            sbi::shutdown(true);
        }
    }

    info!("entering scheduler");
    println!();
    task::run()
}

/// The program the kernel launches, and its arguments.
///
/// nginx runs in the foreground (`daemon off`) so its master process stays as
/// pid 1 and we can see its log output on the console.
const INIT_PATH: &str = "/usr/sbin/nginx";
const INIT_ARGS: &[&str] = &["nginx", "-g", "daemon off;"];
const INIT_ENV: &[&str] = &[
    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "HOME=/root",
    "TERM=linux",
    "LD_LIBRARY_PATH=/lib:/usr/lib",
    "TZ=UTC",
];

fn start_init() -> fs::Result<()> {
    use alloc::vec::Vec;

    let argv: Vec<Vec<u8>> = INIT_ARGS.iter().map(|s| s.as_bytes().to_vec()).collect();
    let envp: Vec<Vec<u8>> = INIT_ENV.iter().map(|s| s.as_bytes().to_vec()).collect();

    // Build the address space and load the executable. `loader::exec` needs a
    // current task (for uid lookups and the aspace), so create the task with a
    // placeholder context first, then load into it.
    let aspace = mm::AddrSpace::new().ok_or(err!(ENOMEM))?;
    let task = task::Task::new_init(aspace, 0, 0);
    *task.group.exe.write() = INIT_PATH.into();
    task::spawn(task.clone());

    // Temporarily make it current so the loader can reach it.
    task::set_current_for_init(task.clone());
    let result = loader::exec(INIT_PATH, &argv, &envp);
    let cx = match result {
        Ok(cx) => cx,
        Err(e) => {
            task::clear_current_for_init();
            return Err(e);
        }
    };
    task.set_trap_context(cx);
    task::clear_current_for_init();

    info!("init: {} {:?}", INIT_PATH, &INIT_ARGS[1..]);
    Ok(())
}

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // A panic inside the panic handler would loop forever; bail out directly.
    if PANICKING.swap(true, Ordering::SeqCst) {
        sbi::shutdown(true);
    }

    console::print_fmt_nolock(format_args!(
        "\n\x1b[1;31m[PANIC]\x1b[0m {}\n",
        info.message()
    ));
    if let Some(location) = info.location() {
        console::print_fmt_nolock(format_args!(
            "        at {}:{}:{}\n",
            location.file(),
            location.line(),
            location.column()
        ));
    }
    let (used, total) = mm::frame::stats();
    console::print_fmt_nolock(format_args!(
        "        frames {}/{}, heap {}/{} bytes\n",
        used,
        total,
        mm::heap::used(),
        mm::heap::total()
    ));
    if task::has_current() {
        let task = task::current();
        console::print_fmt_nolock(format_args!(
            "        current task: pid {} tid {} ({})\n",
            task.pid(),
            task.tid,
            task.name()
        ));
    }
    sbi::shutdown(true)
}
