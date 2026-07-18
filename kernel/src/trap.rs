//! Kernel-mode trap handling (S-mode traps only; no user-mode trampoline yet).

use core::arch::global_asm;
use riscv::register::scause::{self, Exception, Interrupt, Trap};
use riscv::register::{sepc, sie, sstatus, stval, stvec};

global_asm!(include_str!("trap.S"));

#[repr(C)]
#[derive(Debug)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
}

/// Ticks per timer interrupt. CLINT frequency on QEMU virt is 10 MHz,
/// so this fires roughly every 100ms.
const TIMER_INTERVAL: u64 = 1_000_000;

pub fn init() {
    unsafe extern "C" {
        fn __kernel_trap();
    }
    unsafe {
        stvec::write(__kernel_trap as usize, stvec::TrapMode::Direct);
        sie::set_stimer();
        sstatus::set_sie();
    }
    set_next_timer_interrupt();
}

pub fn set_next_timer_interrupt() {
    let now = riscv::register::time::read64();
    sbi_rt::set_timer(now + TIMER_INTERVAL);
}

#[unsafe(no_mangle)]
fn trap_handler(cx: &mut TrapContext) -> &mut TrapContext {
    let cause = scause::read();
    let stval_val = stval::read();
    match cause.cause() {
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_timer_interrupt();
        }
        Trap::Exception(Exception::Breakpoint) => {
            crate::println!("[kernel] breakpoint at {:#x}", cx.sepc);
            cx.sepc += 2;
        }
        _ => {
            panic!(
                "unhandled trap {:?}, stval={:#x}, sepc={:#x}",
                cause.cause(),
                stval_val,
                cx.sepc
            );
        }
    }
    cx
}
