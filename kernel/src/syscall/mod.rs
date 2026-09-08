//! System call dispatch (Linux riscv64 ABI).
use crate::trap::TrapFrame;

pub fn handle(_tf: &mut TrapFrame) {}

pub fn deliver_signal_fault(tf: &mut TrapFrame, _sig: usize) {
    crate::println!("fatal signal for pc={:#x}", tf.sepc);
    crate::task::exit_current(-11);
}
