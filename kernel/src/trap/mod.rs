//! Trap handling: two entry points share the single `stvec` register.
//!
//! - `__kernel_trap` (`kernel_trap.S`): a trap that occurs while the kernel
//!   itself is executing (e.g. a timer interrupt during syscall handling).
//!   No address-space switch is needed; it saves/restores a transient frame
//!   on the current kernel stack and `sret`s straight back.
//! - `__alltraps`/`__restore` (`trampoline.S`): the user<->kernel boundary.
//!   Mapped at the fixed `TRAMPOLINE` virtual address in every address
//!   space so it stays valid across the `satp` switch it performs.
//!
//! `stvec` is repointed between the two (`set_kernel_trap_entry` /
//! `set_user_trap_entry`) at the moments control crosses that boundary.

pub mod context;

use crate::config::{TRAMPOLINE, TRAP_CONTEXT};
use core::arch::global_asm;
use riscv::register::scause::{self, Exception, Interrupt, Trap};
use riscv::register::{sie, sstatus, stval, stvec};

pub use context::TrapContext;

global_asm!(include_str!("kernel_trap.S"));
global_asm!(include_str!("../trampoline.S"));

/// Ticks per timer interrupt. CLINT frequency on QEMU virt is 10 MHz,
/// so this fires roughly every 100ms.
const TIMER_INTERVAL: u64 = 1_000_000;

#[repr(C)]
struct KernelTrapFrame {
    x: [usize; 32],
    sstatus: usize,
    sepc: usize,
}

pub fn init() {
    set_kernel_trap_entry();
    unsafe {
        sie::set_stimer();
        sstatus::set_sie();
    }
    set_next_timer_interrupt();
}

pub fn set_next_timer_interrupt() {
    let now = riscv::register::time::read64();
    sbi_rt::set_timer(now + TIMER_INTERVAL);
}

fn set_kernel_trap_entry() {
    unsafe extern "C" {
        fn __kernel_trap();
    }
    unsafe {
        stvec::write(__kernel_trap as *const () as usize, stvec::TrapMode::Direct);
    }
}

fn set_user_trap_entry() {
    unsafe {
        stvec::write(TRAMPOLINE, stvec::TrapMode::Direct);
    }
}

/// Called by `kernel_trap.S` when a trap occurs while kernel code itself is
/// running. Only expected to see (and simply re-arm) the timer; anything
/// else indicates a real kernel bug.
#[unsafe(no_mangle)]
fn trap_from_kernel_handler(cx: &mut KernelTrapFrame) -> &mut KernelTrapFrame {
    let cause = scause::read();
    let stval_val = stval::read();
    match cause.cause() {
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_timer_interrupt();
        }
        _ => {
            panic!(
                "unhandled trap from kernel mode: {:?}, stval={:#x}, sepc={:#x}",
                cause.cause(),
                stval_val,
                cx.sepc
            );
        }
    }
    cx
}

/// Called (via an indirect jump, not a normal Rust call) by `__alltraps`
/// once it has saved the user context and switched to the kernel address
/// space. Never returns in the usual sense: it always ends by jumping back
/// into `__restore` via [`trap_return`].
#[unsafe(no_mangle)]
fn trap_handler() -> ! {
    set_kernel_trap_entry();
    let cause = scause::read();
    let stval_val = stval::read();
    match cause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            let cx = crate::task::current_trap_cx();
            cx.sepc += 4;
            let syscall_id = cx.x[17];
            let args = [cx.x[10], cx.x[11], cx.x[12], cx.x[13], cx.x[14], cx.x[15]];
            let result = crate::syscall::syscall(syscall_id, args);
            // The syscall may have replaced the address space (execve) or
            // otherwise changed which task is "current" (exit); re-fetch.
            // `rt_sigreturn` (139) is special: it has already restored the
            // *entire* pre-signal context (including a0), so overwriting
            // a0 with its return value here would clobber that restore.
            const SYSCALL_RT_SIGRETURN: usize = 139;
            if syscall_id != SYSCALL_RT_SIGRETURN {
                let cx = crate::task::current_trap_cx();
                cx.x[10] = result as usize;
            }
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault) => {
            let cx = crate::task::current_trap_cx();
            crate::println!(
                "[kernel] pid={} memory fault {:?} at sepc={:#x}, stval(bad addr)={:#x}, ra={:#x} a0={:#x} a1={:#x} a2={:#x} sp={:#x}, killing task",
                crate::task::current_pid(),
                cause.cause(),
                cx.sepc,
                stval_val,
                cx.x[1],
                cx.x[10],
                cx.x[11],
                cx.x[12],
                cx.x[2],
            );
            if cx.sepc == 0 {
                let token = crate::task::current_user_token();
                for base in [cx.x[10], cx.x[11]] {
                    if base == 0 {
                        continue;
                    }
                    let bytes = crate::mm::translated_byte_buffer(token, base as *const u8, 64);
                    crate::print!("[kernel] dump {:#x}:", base);
                    for chunk in bytes {
                        for b in chunk.iter() {
                            crate::print!(" {:02x}", b);
                        }
                    }
                    crate::println!();
                }
            }
            crate::task::exit_current_and_run_next(-1);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            crate::println!(
                "[kernel] pid={} illegal instruction at sepc={:#x}, killing task",
                crate::task::current_pid(),
                crate::task::current_trap_cx().sepc,
            );
            crate::task::exit_current_and_run_next(-1);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_timer_interrupt();
            crate::task::suspend_current_and_run_next();
        }
        _ => {
            panic!(
                "unhandled trap from user mode: {:?}, stval={:#x}",
                cause.cause(),
                stval_val
            );
        }
    }
    trap_return();
}

pub fn trap_return() -> ! {
    match crate::signal::check_and_deliver() {
        crate::signal::SignalAction::Terminate(code) => {
            crate::task::exit_current_and_run_next(code);
        }
        crate::signal::SignalAction::None | crate::signal::SignalAction::Deliver => {}
    }
    set_user_trap_entry();
    let trap_cx_ptr = TRAP_CONTEXT;
    let user_satp = crate::task::current_user_token();
    unsafe extern "C" {
        fn __alltraps();
        fn __restore();
    }
    let restore_va = __restore as usize - __alltraps as usize + TRAMPOLINE;
    unsafe {
        core::arch::asm!(
            "fence.i",
            "jr {restore_va}",
            restore_va = in(reg) restore_va,
            in("a0") trap_cx_ptr,
            in("a1") user_satp,
            options(noreturn)
        );
    }
}
