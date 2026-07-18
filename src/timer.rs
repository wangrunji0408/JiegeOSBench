//! 定时器（riscv time CSR + SBI set_timer）

use crate::config::{CLOCK_FREQ, TICKS_PER_SEC};
use core::arch::asm;

pub fn get_time() -> u64 {
    let t: u64;
    unsafe {
        asm!("rdtime {}", out(reg) t);
    }
    t
}

pub fn get_time_us() -> u64 {
    get_time() * 1_000_000 / CLOCK_FREQ
}

pub fn get_time_ms() -> u64 {
    get_time() * 1_000 / CLOCK_FREQ
}

pub fn set_next_trigger() {
    crate::sbi::set_timer(get_time() + CLOCK_FREQ / TICKS_PER_SEC);
}

pub fn init() {
    // 使能 S 态定时器中断（sie.STIE）
    unsafe {
        asm!("csrs sie, {}", in(reg) 1usize << 5);
    }
    set_next_trigger();
    println!("timer initialized");
}
