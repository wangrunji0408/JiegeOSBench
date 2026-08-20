#![no_std]
#![no_main]
#![allow(clippy::missing_safety_doc)]

#[macro_use]
extern crate alloc;

mod dtb;
mod elf;
mod errno;
mod heap;
mod net;
mod page;
mod pmm;
mod proc;
mod sbi;
mod start;
mod syscall;
mod syscall_nr;
mod trap;
mod uart;
mod vfs;

use core::fmt::{self, Write};

/// 内核版本标识
pub const KERNEL_NAME: &str = "ijiege";

/// 内核结束符号（链接脚本提供）
extern "C" {
    static __kernel_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
}

/// 全局 UART 输出器
pub struct UartWriter;
impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        uart::write_str(s);
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    let mut w = UartWriter;
    let _ = w.write_fmt(args);
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::_print(core::format_args!("\n")) };
    ($($arg:tt)*) => {
        $crate::_print(core::format_args!("[kernel] {}\n", format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::_print(core::format_args!($($arg)*))
    };
}

/// 内核入口（由 start.asm 调用）
#[no_mangle]
extern "C" fn rust_main(hartid: usize, dtb_paddr: usize) -> ! {
    // 清 BSS（start.asm 已做，但保险再判一次）
    uart::early_init();
    static mut ENTER_COUNT: u32 = 0;
    unsafe {
        ENTER_COUNT += 1;
        let ec = ENTER_COUNT;
        core::ptr::write_volatile(core::ptr::addr_of_mut!(ENTER_COUNT), ec);
    }
    let ec = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTER_COUNT)) };
    kprintln!("ijiege kernel booting, hartid={}, dtb={:#x}, enter={}", hartid, dtb_paddr, ec);
    if ec > 1 {
        // 打印 dtb 前 32 字节帮助诊断
        let mut i = 0;
        let mut line = alloc::string::String::from("dtb bytes:");
        while i < 32 {
            let b = unsafe { ((dtb_paddr + i) as *const u8).read_volatile() };
            use core::fmt::Write;
            let _ = write!(line, " {:02x}", b);
            i += 1;
        }
        kprintln!("{}", line);
    }

    // 1. 解析设备树：内存大小
    let (mem_start, mem_size) = dtb::parse_memory(dtb_paddr);
    let mem_end = mem_start + mem_size;
    kprintln!("memory: {:#x} - {:#x} ({} MB)", mem_start, mem_end, mem_size >> 20);

    // 2. 物理内存管理器（跳过内核镜像占用的部分）
    let kernel_end = unsafe { &__kernel_end as *const u8 as usize };
    let heap_start = (kernel_end + 0xfff) & !0xfff;
    let heap_size = 32 << 20; // 32MB 内核堆
    heap::init(heap_start, heap_size); // 堆必须先于任何 Vec 分配
    pmm::init(heap_start + heap_size, mem_end);
    kprintln!(
        "kernel image ends {:#x}, heap {:#x}-{:#x}, pages from {:#x}",
        kernel_end, heap_start, heap_start + heap_size, heap_start + heap_size
    );

    // 3. 初始化网络（virtio-net + smoltcp）
    net::init();

    // 4. 初始化 trap / 装载用户进程
    trap::init();

    // 5. 挂载 rootfs 并启动第一个用户程序
    vfs::init();
    let argv: [&str; 3] = ["/bin/busybox.static", "echo", "hello from busybox static"];
    proc::spawn(&argv).expect("failed to spawn init process");

    unreachable!("spawn returned")
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kprintln!("\n[panic] {}", info);
    kprintln!("system halted.");
    sbi::shutdown()
}
