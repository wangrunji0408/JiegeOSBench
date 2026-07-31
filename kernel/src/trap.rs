//! Trap handler: dispatch interrupts and exceptions, drive the scheduler.

use crate::console::kprintln;
use crate::task::TrapFrame;

#[no_mangle]
pub extern "C" fn trap_handler(
    tf: *mut TrapFrame,
    scause: usize,
    sepc: usize,
    stval: usize,
) -> *mut TrapFrame {
    let from_user = unsafe { (*tf).from_user() };
    let is_interrupt = scause & (1usize << 63) != 0;

    if is_interrupt {
        let code = scause & !(1usize << 63);
        match code {
            5 => {
                // supervisor timer interrupt
                crate::timer::on_timer_interrupt();
                if from_user {
                    // preempt if another task is ready
                    if !crate::task::READY.is_empty() {
                        crate::task::schedule();
                    }
                }
            }
            9 => {
                // supervisor external interrupt (PLIC)
                let irq = crate::plic::claim();
                if irq == crate::virtio::device_irq() {
                    // virtio-net
                    crate::virtio::irq_handler();
                    crate::plic::complete(irq);
                } else if irq != 0 {
                    crate::plic::complete(irq);
                }
            }
            1 => {
                // supervisor software interrupt (no IPIs; ignore)
            }
            _ => {
                kprintln!("[trap] unknown interrupt scause={:#x}", scause);
            }
        }
    } else {
        match scause {
            8 => {
                // ecall from U-mode: syscall
                crate::syscall::handle(tf);
                // deliver pending signals before returning to user
                if unsafe { (*tf).from_user() } {
                    crate::signal::maybe_deliver(tf);
                }
            }
            12 | 13 | 15 => {
                // instruction/page fault
                let pid = crate::task::current_pid();
                if !from_user {
                    kprintln!(
                        "[trap] KERNEL page fault scause={} sepc={:#x} stval={:#x}",
                        scause, sepc, stval
                    );
                    panic!("kernel fault");
                }
                kprintln!(
                    "[trap] pid={} user fault scause={} sepc={:#x} stval={:#x} -> killed",
                    pid, scause, sepc, stval
                );
                crate::task::exit(128 + 11); // SIGSEGV
            }
            2 => {
                // illegal instruction
                let pid = crate::task::current_pid();
                kprintln!(
                    "[trap] pid={} illegal instruction sepc={:#x} stval={:#x} -> killed",
                    pid, sepc, stval
                );
                crate::task::exit(128 + 4); // SIGILL
            }
            0 => {
                // instruction address misaligned
                kprintln!("[trap] misaligned instruction at {:#x}", sepc);
                crate::task::exit(128 + 4);
            }
            9 => {
                // ecall from S-mode (should not happen; SBI handled by M-mode)
                kprintln!("[trap] ecall from S-mode");
            }
            _ => {
                kprintln!(
                    "[trap] unhandled exception scause={} sepc={:#x} stval={:#x}",
                    scause, sepc, stval
                );
                if !from_user {
                    panic!("kernel exception");
                }
                crate::task::exit(128 + 4);
            }
        }
    }

    // If a signal arrived while we were in the kernel (e.g. from another task),
    // deliver it before returning to user.
    if from_user && !is_interrupt {
        crate::signal::maybe_deliver(tf);
    }

    tf
}
