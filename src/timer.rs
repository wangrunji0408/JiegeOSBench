//! 时钟：通过 SBI 设置下次中断，提供 tick 计数与调度驱动。

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sbi;

static TICKS: AtomicU64 = AtomicU64::new(0);

const CLOCK_FREQ: usize = 10_000_000; // QEMU virt: 10 MHz
const INTERVAL: usize = CLOCK_FREQ / 100; // 100 Hz

/// 读取 m-mode time CSR（S-mode 下通过 rdtime 指令读）
#[inline]
pub fn read_cycles() -> usize {
    let t: usize;
    unsafe {
        core::arch::asm!("rdtime {}", out(reg) t);
    }
    t
}

pub fn read_time_us() -> usize {
    read_cycles() / (CLOCK_FREQ / 1_000_000)
}

pub fn next_interrupt() {
    let t = read_cycles();
    sbi::set_timer(t + INTERVAL);
}

pub fn init() {
    next_interrupt();
}

pub fn tick() {
    let t = TICKS.fetch_add(1, Ordering::SeqCst);
    if t % 100 == 0 {
        crate::println!("[timer] {}s", t / 100);
    }
    next_interrupt();
    crate::sched::on_tick();
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::SeqCst)
}
