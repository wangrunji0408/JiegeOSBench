//! Trap 处理：保存上下文、分发异常/中断、调用处理函数后恢复。

use core::arch::global_asm;
use crate::println;

global_asm!(include_str!("trap.S"));

/// Trap 上下文，与 trap.S 的栈布局严格对应（36 字段 = 288 字节，栈分配 304）。
/// 偏移：x[0..32] -> 0..31*8，sepc=32*8, sstatus=33*8, stval=34*8, scause=35*8
#[repr(C)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
    pub stval: usize,
    pub scause: usize,
}

/// sstatus 位
pub const SSTATUS_SPP: usize = 1 << 8; // 1=trap 来自 S-mode, 0=来自 U-mode
pub const SSTATUS_SPIE: usize = 1 << 5; // sret 后开中断
pub const SSTATUS_SUM: usize = 1 << 18; // S-mode 可访问用户页
pub const SSTATUS_FS_INITIAL: usize = 0b01 << 13; // 启用 FPU
pub const SSTATUS_FS_DIRTY: usize = 0b11 << 13;

impl TrapContext {
    /// 构造用户态初始上下文
    pub fn new_user_entry(entry: usize, user_sp: usize, kstack_top: usize) -> Self {
        Self {
            x: {
                let mut r = [0usize; 32];
                r[2] = user_sp; // sp
                r
            },
            sepc: entry,
            sstatus: SSTATUS_SPIE | SSTATUS_SUM, // SPP=0(U), SPIE=1, SUM=1(内核可访问用户页)
            stval: 0,
            scause: 0,
        }
    }

    /// 构造内核态初始上下文（idle 等）
    pub fn new_kernel_entry(entry: usize, sp: usize) -> Self {
        Self {
            x: {
                let mut r = [0usize; 32];
                r[2] = sp - 304; // __restore 末尾 ld sp,2*8(sp)；这里存的是恢复后的 sp
                r
            },
            sepc: entry,
            sstatus: SSTATUS_SPP | SSTATUS_SPIE, // S-mode
            stval: 0,
            scause: 0,
        }
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
            5 => crate::timer::tick(),      // S-mode timer
            1 => unsafe {                   // S-mode software
                core::arch::asm!("csrc sip, {}", in(reg) 0x2_usize);
            },
            9 => crate::irq::external(),    // S-mode external
            _ => {
                println!("[trap] unknown interrupt code {}", code);
            }
        }
    } else {
        match code {
            8 => {
                // ecall from U-mode
                cx.sepc += 4;
                crate::syscall::do_syscall(cx);
            }
            9 => {
                println!("[trap] ecall from S-mode (unexpected)");
                cx.sepc += 4;
            }
            12 => println!("[trap] instr page fault @ {:#x} stval={:#x}", cx.sepc, cx.stval),
            13 => println!("[trap] load page fault @ {:#x} stval={:#x}", cx.sepc, cx.stval),
            15 => println!("[trap] store page fault @ {:#x} stval={:#x}", cx.sepc, cx.stval),
            2 => println!("[trap] illegal instr @ {:#x}", cx.sepc),
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
