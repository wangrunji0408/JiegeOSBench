//! 时钟：通过 SBI 设置下次中断，提供 tick 计数与调度驱动。

use core::sync::atomic::{AtomicU64, Ordering};
use riscv::register::time;
use crate::sbi;

static TICKS: AtomicU64 = AtomicU64::new(0);

const CLOCK_FREQ: usize = 10_000_000; // QEMU virt: 10 MHz
const INTERVAL: usize = CLOCK_FREQ / 100; // 100 Hz

pub fn read_time() -> usize {
    cycles_to_us(time::read())
}

pub fn cycles_to_us(cycles: usize) -> usize {
    cycles / (CLOCK_FREQ / 1_000_000)
}

pub fn next_interrupt() {
    let t = time::read();
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
    // 让调度器有机会切换（Phase 4 接入）
    crate::sched::on_tick();
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::SeqCst)
}
