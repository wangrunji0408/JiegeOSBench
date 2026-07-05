//! 智能杰哥 RISC-V 内核主入口。
#![no_std]
#![no_main]
#![feature(asm_const)]

mod uart;
mod sbi;
mod console;
mod trap;
mod timer;
mod irq;
mod syscall;
mod sched;

use core::arch::{global_asm, asm};

// 引入启动汇编
global_asm!(include_str!("entry.S"));

// 链接脚本提供的内核镜像结束符号
extern "C" {
    fn __kernel_end();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("[kernel panic] {info}");
    sbi::shutdown();
}

/// 设置 stvec 指向 trap 入口
fn set_stvec() {
    extern "C" {
        fn __alltraps();
    }
    unsafe {
        asm!("csrw stvec, {}", in(reg) __alltraps as usize);
    }
}

/// 启用 S-mode 中断（SIE）与时钟中断
fn enable_interrupts() {
    unsafe {
        // sstatus.SIE = 1
        asm!("csrsi sstatus, 0x2");
        // sie.STIE (timer) + sie.SSIE (soft) + sie.SEIE (external)
        asm!("csrs sie, {}", in(reg) 0x226_usize);
    }
}

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    uart::init();
    println!();
    println!("==============================================");
    println!("  智能杰哥 OS (ijiege-os) booted on RISC-V");
    println!("  target: run nginx official binary in QEMU");
    println!("==============================================");
    println!("[boot] UART initialized");
    println!("[boot] kernel image end = {:#x}", __kernel_end as *const () as usize);

    set_stvec();
    println!("[boot] stvec set");
    timer::init();
    println!("[boot] timer armed @ 100Hz");
    enable_interrupts();
    println!("[boot] interrupts enabled");

    println!("[boot] idle loop");
    let mut n: u64 = 0;
    loop {
        unsafe { asm!("wfi"); }
        n += 1;
        if n % 50 == 0 {
            println!("[idle] wfi woke {} times, ticks={}", n, timer::ticks());
        }
    }
}
