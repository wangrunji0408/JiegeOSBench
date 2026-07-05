//! 智能杰哥 RISC-V 内核主入口。
#![no_std]
#![no_main]
#![feature(asm_const)]

extern crate alloc;

mod uart;
mod sbi;
mod console;
mod trap;
mod timer;
mod irq;
mod syscall;
mod sched;
mod task;
mod mm;
mod elf;
mod process;

use core::arch::{global_asm, asm};
use alloc::boxed::Box;

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

    // 初始化内存管理：帧分配器 + 内核堆 + Sv39 页表
    mm::init();

    // 堆分配自检
    let p: Box<u64> = Box::new(0x1234_5678_9ABC_DEF0);
    println!("[boot] heap test: Box<u64> = {:#x} @ {:p}", *p, p);
    let mut v = alloc::vec![1u32, 2, 3, 4, 5];
    v.push(6);
    println!("[boot] heap test: vec = {:?} (len {})", v, v.len());
    drop(p);
    drop(v);

    set_stvec();
    println!("[boot] stvec set");
    timer::init();
    println!("[boot] timer armed @ 100Hz");
    enable_interrupts();
    println!("[boot] interrupts enabled");

    // 加载并运行用户态测试程序
    static HELLO_ELF: &[u8] = include_bytes!("../user/hello.elf");
    static LOOP_ELF: &[u8] = include_bytes!("../user/loop.elf");
    sched::spawn(HELLO_ELF, "hello");
    sched::spawn(LOOP_ELF, "loopA");
    sched::spawn(LOOP_ELF, "loopB");

    println!("[boot] entering scheduler...");
    sched::run_first_task();
}
