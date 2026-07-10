use riscv::register::{time, sie};

use crate::config::{CLOCK_FREQ, TIME_SLICE_MS};

pub fn init() {
    set_next_timer();
    unsafe { sie::set_stimer(); }
}

pub fn get_time_ms() -> usize {
    time::read() / (CLOCK_FREQ / 1000)
}

pub fn get_time_us() -> usize {
    time::read() / (CLOCK_FREQ / 1_000_000)
}

pub fn set_next_timer() {
    let current = time::read();
    let interval = CLOCK_FREQ * TIME_SLICE_MS / 1000;
    sbi_set_timer(current + interval);
}

fn sbi_set_timer(stime_value: usize) {
    unsafe {
        core::arch::asm!(
            "li a7, 0x54494D45",
            "li a6, 0",
            "mv a0, {0}",
            "ecall",
            in(reg) stime_value,
            out("a0") _,
            lateout("a1") _,
            options(nomem, nostack)
        );
    }
}

pub fn tick() {
    set_next_timer();
}
