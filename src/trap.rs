//! Trap 处理：保存上下文、分发异常/中断、调用处理函数后恢复。

use core::arch::global_asm;
use crate::println;

global_asm!(include_str!("trap.S"));

/// Trap 上下文，与 trap.S 的栈布局严格对应。
/// 偏移：x1..x31 在 0..31*8，sepc=31*8, sstatus=32*8, stval=33*8, scause=34*8, sscratch=35*8
#[repr(C)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
    pub stval: usize,
    pub scause: usize,
    pub sscratch: usize,
}

impl TrapContext {
    pub fn new_kernel_entry(entry: usize, sp: usize) -> Self {
        let mut ctx = Self {
            x: [0; 32],
            sepc: entry,
            sstatus: 0,
            stval: 0,
            scause: 0,
            sscratch: 0,
        };
        // SPP=1 表示返回到 S-mode
        ctx.sstatus = 1 << 8; // SPP
        ctx.x[2] = sp; // sp
        ctx
    }
}

const CAUSE_INTERRUPT_BIT: usize = 1 << (core::mem::size_of::<usize>() * 8 - 1);

#[no_mangle]
pub fn trap_handler(cx: &mut TrapContext) -> &mut TrapContext {
    let scause = cx.scause;
    let is_interrupt = (scause & CAUSE_INTERRUPT_BIT) != 0;
    let code = scause & !CAUSE_INTERRUPT_BIT;

    if is_interrupt {
        match code {
            // Supervisor timer interrupt
            5 => {
                crate::timer::tick();
            }
            // Supervisor software interrupt
            1 => {
                // 清 pending
                unsafe {
                    core::arch::asm!("csrc sip, {}", in(reg) 0x2_usize);
                }
            }
            // Supervisor external interrupt
            9 => {
                crate::irq::external();
            }
            _ => {
                println!("[trap] unknown interrupt code {}", code);
            }
        }
    } else {
        match code {
            // Environment call from U-mode
            8 => {
                cx.sepc += 4;
                crate::syscall::do_syscall(cx);
            }
            // Environment call from S-mode
            9 => {
                println!("[trap] ecall from S-mode (unexpected)");
                cx.sepc += 4;
            }
            // Instruction page fault
            12 => {
                println!(
                    "[trap] instruction page fault @ {:#x}, stval={:#x}",
                    cx.sepc, cx.stval
                );
            }
            // Load page fault
            13 => {
                println!(
                    "[trap] load page fault @ {:#x}, stval={:#x}",
                    cx.sepc, cx.stval
                );
            }
            // Store/AMO page fault
            15 => {
                println!(
                    "[trap] store page fault @ {:#x}, stval={:#x}",
                    cx.sepc, cx.stval
                );
            }
            // Illegal instruction
            2 => {
                println!("[trap] illegal instruction @ {:#x}", cx.sepc);
            }
            // Breakpoint
            3 => {
                println!("[trap] breakpoint @ {:#x}", cx.sepc);
                cx.sepc += 2;
            }
            _ => {
                println!("[trap] unknown exception code {}", code);
            }
        }
    }
    cx
}
