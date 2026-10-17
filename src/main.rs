#![no_std]
#![no_main]
#![allow(static_mut_refs)]
#![allow(dead_code)]

extern crate alloc;

use core::arch::global_asm;

mod arch;
mod elf;
mod fs;
mod heap;
mod mm;
mod net;
mod sbi;
mod syscall;
mod syscall_nr;
mod trap;
mod uart;
mod virtio;
mod user;

global_asm!(
    r#"
    .section .text.entry
    .global _start
_start:
    la      sp, boot_stack_top
    la      t0, __bss_start
    la      t1, __bss_end
1:
    bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b
2:
    call    kmain
3:
    wfi
    j       3b

    .section .bss
    .align 12
boot_stack:
    .space 131072
boot_stack_top:
"#
);

pub static mut TRACE: u32 = 0;

pub fn trace_syscall(nr: usize, path: &[u8]) {
    unsafe {
        if TRACE >= 1 && nr == syscall_nr::OPENAT {
            println!(
                "[open] {}",
                core::str::from_utf8(path).unwrap_or("<non-utf8>")
            );
        }
    }
}

fn dtb_total_size(dtb: usize) -> usize {
    if dtb == 0 {
        return 0;
    }
    unsafe {
        let p = dtb as *const u8;
        if *p != 0xd0 || *p.add(1) != 0x0d || *p.add(2) != 0xfe || *p.add(3) != 0xed {
            return 0;
        }
        let sz = ((*p.add(4) as usize) << 24)
            | ((*p.add(5) as usize) << 16)
            | ((*p.add(6) as usize) << 8)
            | (*p.add(7) as usize);
        sz.min(4 * 1024 * 1024)
    }
}

fn find_arg(dtb: usize, key: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let sz = dtb_total_size(dtb);
    if sz == 0 {
        return None;
    }
    let bytes = unsafe { core::slice::from_raw_parts(dtb as *const u8, sz) };
    let mut i = 0;
    while i + key.len() < bytes.len() {
        if &bytes[i..i + key.len()] == key {
            let mut v = alloc::vec::Vec::new();
            let mut j = i + key.len();
            while j < bytes.len() && bytes[j] != 0 && bytes[j] != b' ' {
                v.push(bytes[j]);
                j += 1;
            }
            return Some(v);
        }
        i += 1;
    }
    None
}

#[no_mangle]
pub extern "C" fn kmain(hartid: usize, dtb: usize) -> ! {
    uart::init();
    heap::init();
    fs::init();
    mm::init();
    arch::init_trap();
    net::init();

    unsafe {
        syscall::BOOT_WALL_NS = 1_767_225_600u64 * 1_000_000_000; // 2026-01-01
    }

    println!("==============================================");
    println!("  jiege-os - a RISC-V Linux-compatible kernel");
    println!("  hartid={} dtb={:#x}", hartid, dtb);
    println!(
        "  rootfs {} bytes, {} nodes",
        fs::ROOTFS.len(),
        fs::nodes().len()
    );
    println!("==============================================");

    let trace = find_arg(dtb, b"JTRACE=");
    if let Some(t) = trace {
        unsafe {
            TRACE = t.first().map(|c| c - b'0').unwrap_or(0) as u32;
        }
    }

    let prog = find_arg(dtb, b"JPROG=").unwrap_or_else(|| b"/sbin/nginx".to_vec());
    println!(
        "[boot] program: {}",
        core::str::from_utf8(&prog).unwrap_or("?")
    );

    let mut argv_storage: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
    if let Some(a) = find_arg(dtb, b"JARG=") {
        for part in a.split(|&b| b == b',') {
            argv_storage.push(part.to_vec());
        }
    } else if prog == b"/bin/busybox" {
        argv_storage.push(b"busybox".to_vec());
        argv_storage.push(b"echo".to_vec());
        argv_storage.push(b"hello from userspace".to_vec());
    } else {
        argv_storage.push(b"nginx".to_vec());
    }
    let mut argv: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
    for a in &argv_storage {
        argv.push(a);
    }

    let env_storage: [&[u8]; 5] = [
        b"PATH=/bin:/sbin:/usr/bin:/usr/sbin",
        b"HOME=/",
        b"TMPDIR=/tmp",
        b"USER=root",
        b"LANG=C",
    ];
    let mut envp: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
    for e in env_storage.iter() {
        envp.push(e);
    }

    user::exec(&prog, &argv, &envp);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[panic] {}", info);
    sbi::shutdown(true);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::uart::UartWriter, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        $crate::print!($($arg)*);
        $crate::print!("\n");
    }};
}
