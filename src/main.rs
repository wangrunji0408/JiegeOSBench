//! 智能杰哥 RISC-V 内核主入口。
#![no_std]
#![no_main]


mod uart;
mod sbi;
mod console;

use core::arch::global_asm;

// 引入启动汇编
global_asm!(include_str!("entry.S"));

/// 链接脚本提供的内核镜像结束符号
extern "C" {
    fn __kernel_end();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(msg) = info.message() {
        println!("[kernel panic] {} at {}", msg, info.location().unwrap());
    } else {
        println!("[kernel panic] at {}", info.location().unwrap());
    }
    sbi::shutdown();
}

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    uart::init();
    println!();
    println!("==============================================");
    println!("  智能杰哥 OS (ijiege-os) booted on RISC-V");
    println!("  target: run nginx official binary in QEMU");
    println!("==============================================");
    println!("[boot] UART NS16550A @ 0x10000000 initialized");
    println!("[boot] kernel image end = {:#x}", __kernel_end as usize);

    // Phase 1 验证点：能进到这里并持续输出即成功
    let mut n: u64 = 0;
    loop {
        if n % 10_000_000 == 0 {
            println!("[alive] tick {}", n / 10_000_000);
        }
        n += 1;
    }
}
