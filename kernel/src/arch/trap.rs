use core::arch::global_asm;
use riscv::register::{stvec, scause, stval, sepc, sscratch};

use crate::arch::context::TrapContext;
use crate::syscall::syscall;
use crate::task;
use crate::timer;

global_asm!(include_str!("riscv64/trap.S"));

pub fn init() {
    extern "C" { fn __alltraps(); }
    unsafe {
        stvec::write(__alltraps as usize, stvec::TrapMode::Direct);
    }
    unsafe { sscratch::write(0); }
}

/// 设置当前任务的内核栈（存入sscratch，陷入时用）
pub fn set_kernel_stack(sp: usize) {
    unsafe { sscratch::write(sp); }
}

// 异常码（来自RISC-V特权级规范）
const EXC_INSTRUCTION_MISALIGNED: usize = 0;
const EXC_INSTRUCTION_ACCESS_FAULT: usize = 1;
const EXC_ILLEGAL_INSTRUCTION: usize = 2;
const EXC_BREAKPOINT: usize = 3;
const EXC_LOAD_MISALIGNED: usize = 4;
const EXC_LOAD_ACCESS_FAULT: usize = 5;
const EXC_STORE_MISALIGNED: usize = 6;
const EXC_STORE_ACCESS_FAULT: usize = 7;
const EXC_USER_ECALL: usize = 8;
const EXC_SUPERVISOR_ECALL: usize = 9;
const EXC_INSTRUCTION_PAGE_FAULT: usize = 12;
const EXC_LOAD_PAGE_FAULT: usize = 13;
const EXC_STORE_PAGE_FAULT: usize = 15;

// 中断码
const INT_SUPERVISOR_TIMER: usize = 5;
const INT_SUPERVISOR_EXTERNAL: usize = 9;
const INT_SUPERVISOR_SOFTWARE: usize = 1;

#[no_mangle]
pub fn trap_handler(cx: &mut TrapContext) -> &mut TrapContext {
    let scause = scause::read();
    let stval = stval::read();
    let code = scause.code();

    if scause.is_interrupt() {
        match code {
            INT_SUPERVISOR_TIMER => {
                timer::tick();
                task::schedule();
            }
            INT_SUPERVISOR_EXTERNAL => {
                crate::drivers::handle_external_interrupt();
            }
            INT_SUPERVISOR_SOFTWARE => {
                unsafe { riscv::register::sip::clear_ssoft(); }
            }
            _ => {
                println!("[intr] code={} sepc={:#x}", code, cx.sepc);
            }
        }
    } else {
        match code {
            EXC_USER_ECALL => {
                cx.sepc += 4;
                let args = cx.args();
                let id = cx.syscall_id();
                let result = syscall(id, args, cx);
                match id {
                    34 => println!("[nginx] mkdirat -> {}", result as i32),
                    200 => println!("[nginx] bind fd={} -> {}", args[0] as i32, result as i32),
                    201 => println!("[nginx] listen fd={} -> {}", args[0] as i32, result as i32),
                    220 => println!("[fork] clone flags={:#x}", args[0]),
                    22 => if result < 0 { println!("[epoll_pwait] ERROR -> {}", result as i32); },
                    _ => {}
                }
                cx.set_a0(result as usize);
            }
            EXC_STORE_ACCESS_FAULT | EXC_STORE_PAGE_FAULT => {
                let is_user_addr = cx.sepc < 0x80000000;
                if is_user_addr {
                    task::current_task_exit(-11);
                } else {
                    let pid = crate::task::manager::TASK_MANAGER.lock().current.take();
                    if let Some(pid) = pid {
                        let mut mgr = crate::task::manager::TASK_MANAGER.lock();
                        if let Some(task) = mgr.tasks.get(&pid) {
                            task.lock().state = crate::task::process::TaskState::Zombie(-11);
                        }
                    }
                    crate::task::schedule();
                }
            }
            EXC_LOAD_ACCESS_FAULT | EXC_LOAD_PAGE_FAULT => {
                let is_user_addr = cx.sepc < 0x80000000;
                if stval < 0x1000 {
                    // NULL/small pointer dereference
                    if is_user_addr {
                        task::current_task_exit(-11);
                    } else {
                        // Kernel NULL deref - kill current task
                        let pid = crate::task::manager::TASK_MANAGER.lock().current.take();
                        if let Some(pid) = pid {
                            let mut mgr = crate::task::manager::TASK_MANAGER.lock();
                            if let Some(task) = mgr.tasks.get(&pid) {
                                task.lock().state = crate::task::process::TaskState::Zombie(-11);
                            }
                        }
                        crate::task::schedule();
                    }
                } else {
                    println!("[trap] Load page fault sepc={:#x} stval={:#x}", cx.sepc, stval);
                    if is_user_addr {
                        task::current_task_exit(-11);
                    } else {
                        println!("[trap] KERNEL page fault! Halting.");
                        loop { unsafe { core::arch::asm!("wfi") } }
                    }
                }
            }
            EXC_ILLEGAL_INSTRUCTION => {
                // When nginx calls through a corrupted IFUNC function pointer (GOT),
                // the PLT uses jalr t1, t3 which stores return address in t1 (x6)
                let t1 = cx.x[6]; // t1 = actual return address from PLT
                let ra = cx.x[1]; // ra = previous return address (may be corrupted)
                static FIRST_ILLEGAL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
                if !FIRST_ILLEGAL.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    println!("[trap] Illegal instr at sepc={:#x} t1={:#x} ra={:#x}", cx.sepc, t1, ra);
                    // Check nginx's GOT[memcpy] - at nginx_base + 0x1202a8 = 0x401202a8
                    if let Some(task) = crate::task::current_task() {
                        let t = task.lock();
                        let got_memcpy_va: usize = 0x401202a8;
                        if let Some(pa) = t.memory_set.page_table.translate_va(got_memcpy_va) {
                            let val = unsafe { *(pa as *const u64) };
                            println!("[trap] nginx GOT[memcpy] = {:#x}", val);
                        }
                    }
                }

                // Try to find a valid return address (must be in nginx/library range)
                let return_addr = if t1 >= 0x40000000 && t1 < 0x60000000 && t1 != cx.sepc {
                    Some(t1)
                } else if ra >= 0x40000000 && ra < 0x60000000 && ra != cx.sepc {
                    Some(ra)
                } else {
                    None
                };

                if let Some(ret_addr) = return_addr {
                    cx.sepc = ret_addr;
                    cx.x[10] = 0; // return 0
                } else {
                    // No valid return address - kill process
                    let pid = crate::task::manager::TASK_MANAGER.lock().current.take();
                    if let Some(pid) = pid {
                        let mut mgr = crate::task::manager::TASK_MANAGER.lock();
                        if let Some(task) = mgr.tasks.get(&pid) {
                            task.lock().state = crate::task::process::TaskState::Zombie(-4);
                        }
                    }
                    crate::task::schedule();
                }
            }
            EXC_INSTRUCTION_PAGE_FAULT | EXC_INSTRUCTION_ACCESS_FAULT => {
                // Instruction fault - trying to execute non-executable/inaccessible memory
                let is_user_addr = cx.sepc >= 0x40000000 && cx.sepc < 0x78000000;
                static FIRST_INSTR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
                if !FIRST_INSTR.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    println!("[trap] Instr fault sepc={:#x} ra={:#x} t1={:#x}", cx.sepc, cx.x[1], cx.x[6]);
                }
                if is_user_addr {
                    // Try to return from the corrupted function call (same as Illegal Instruction)
                    let t1 = cx.x[6];
                    let ra = cx.x[1];
                    let return_addr = if t1 >= 0x40000000 && t1 < 0x60000000 && t1 != cx.sepc {
                        Some(t1)
                    } else if ra >= 0x40000000 && ra < 0x60000000 && ra != cx.sepc {
                        Some(ra)
                    } else {
                        None
                    };
                    if let Some(ret_addr) = return_addr {
                        cx.sepc = ret_addr;
                        cx.x[10] = 0;
                    } else {
                        task::current_task_exit(-11);
                    }
                } else {
                    // KERNEL instruction fault - kill current task and reschedule
                    let pid = crate::task::manager::TASK_MANAGER.lock().current.take();
                    if let Some(pid) = pid {
                        let mut mgr = crate::task::manager::TASK_MANAGER.lock();
                        if let Some(task) = mgr.tasks.get(&pid) {
                            task.lock().state = crate::task::process::TaskState::Zombie(-11);
                        }
                    }
                    crate::task::schedule();
                }
            }
            EXC_INSTRUCTION_MISALIGNED => {
                task::current_task_exit(-7);
            }
            _ => {
                println!("[trap] Unhandled exception: code={}, stval={:#x}, sepc={:#x}",
                    code, stval, cx.sepc);
                task::current_task_exit(-1);
            }
        }
    }
    cx
}
