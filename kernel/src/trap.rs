//! Trap handler: dispatch interrupts and exceptions, drive the scheduler.

use crate::kprintln;
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
    // keep the kernel non-preemptible: returning to S-mode must NOT re-enable SIE
    if !from_user {
        unsafe {
            (*tf).sstatus &= !(1 << 5); // clear SPIE
        }
    }

    if is_interrupt {
        let code = scause & !(1usize << 63);
        match code {
            5 => {
                // supervisor timer interrupt
                if from_user {
                    let s0 = unsafe { (*tf).regs[8] };
                    unsafe {
                        crate::task::LAST_TIMER_S0 = s0;
                        crate::task::LAST_TIMER_SEPC = sepc;
                    }
                }
                crate::timer::on_timer_interrupt();
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
                unsafe {
                    (*tf).sepc += 4; // skip the ecall instruction
                }
                crate::syscall::handle(tf);
                // deliver pending signals before returning to user
                if unsafe { (*tf).from_user() } {
                    crate::signal::maybe_deliver(tf);
                }
            }
            12 | 13 | 15 => {
                // instruction/page fault
                if !from_user {
                    kprintln!(
                        "[trap] KERNEL page fault scause={} sepc={:#x} stval={:#x} [prev syscall {}]",
                        scause, sepc, stval,
                        unsafe { crate::syscall::LAST_SYSCALL_NUM }
                    );
                    let tf2 = unsafe { &*tf };
                    for r in 0..32 {
                        kprintln!("  x{}={:#018x}", r, tf2.regs[r]);
                    }
                    if let Some(t) = unsafe { crate::task::current().as_ref() } {
                        if let Some(phys) = t.mm.pt.translate(stval) {
                            crate::kprintln!("  [pt] stval={:#x} -> {:#x}", stval, phys);
                        } else {
                            crate::kprintln!("  [pt] stval={:#x} NOT MAPPED", stval);
                        }
                        for v in &t.mm.vmas {
                            crate::kprintln!("  vma [{:#x}, {:#x}) prot={:#x} anon={} brk={:#x}",
                                v.start, v.end, v.prot, v.anon, t.mm.brk);
                        }
                    }
                    panic!("kernel fault");
                }
                let pid = crate::task::current_pid();
                let usp = unsafe { (*tf).sp() };
                kprintln!(
                    "[trap] pid={} user fault scause={} sepc={:#x} stval={:#x} usp={:#x} -> killed",
                    pid, scause, sepc, stval, usp
                );
                let tf2 = unsafe { &*tf };
                for r in 0..32 {
                    kprintln!("  x{}={:#018x}", r, tf2.regs[r]);
                }
                crate::kprintln!(
                    "  [prev syscall {} sp={:#x}]",
                    unsafe { crate::syscall::LAST_SYSCALL_NUM },
                    unsafe { crate::syscall::LAST_SYSCALL_SP }
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
                if let Some(phys) = unsafe { crate::task::current().as_ref() }.unwrap().mm.pt.translate(sepc) {
                    let bytes = unsafe { core::slice::from_raw_parts(phys as *const u8, 16) };
                    kprintln!("[trap] code at sepc: {:02x?}", bytes);
                }
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
