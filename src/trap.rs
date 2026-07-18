//! Trap 处理：trampoline、用户上下文、内核异常处理

use crate::config::{PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT};
use core::arch::{asm, global_asm};

/// TrapContext 布局（与汇编保持一致）：
/// x0..x31: 0..32  (x0 槽位未用)
/// sstatus: 32, sepc: 33, kernel_satp: 34, kernel_sp: 35, trap_handler: 36
pub const CTX_SIZE: usize = 37 * 8;

#[repr(C)]
#[derive(Clone)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    pub kernel_satp: usize,
    pub kernel_sp: usize,
    pub trap_handler: usize,
}

impl TrapContext {
    pub fn zero() -> Self {
        Self {
            x: [0; 32],
            sstatus: 0,
            sepc: 0,
            kernel_satp: 0,
            kernel_sp: 0,
            trap_handler: 0,
        }
    }
}

global_asm!(
    r#"
    .section .text.trampoline
    .globl __alltraps
    .globl __restore
    .align 2
__alltraps:
    # 此时 satp 仍是用户页表，sp 是用户栈，sscratch = TrapContext 的用户 VA
    csrrw sp, sscratch, sp
    # sp -> TrapContext（用户 VA，S 态可访问 U=0 页）
    sd x1, 1*8(sp)
    sd x3, 3*8(sp)
    sd x4, 4*8(sp)
    sd x5, 5*8(sp)
    sd x6, 6*8(sp)
    sd x7, 7*8(sp)
    sd x8, 8*8(sp)
    sd x9, 9*8(sp)
    sd x10, 10*8(sp)
    sd x11, 11*8(sp)
    sd x12, 12*8(sp)
    sd x13, 13*8(sp)
    sd x14, 14*8(sp)
    sd x15, 15*8(sp)
    sd x16, 16*8(sp)
    sd x17, 17*8(sp)
    sd x18, 18*8(sp)
    sd x19, 19*8(sp)
    sd x20, 20*8(sp)
    sd x21, 21*8(sp)
    sd x22, 22*8(sp)
    sd x23, 23*8(sp)
    sd x24, 24*8(sp)
    sd x25, 25*8(sp)
    sd x26, 26*8(sp)
    sd x27, 27*8(sp)
    sd x28, 28*8(sp)
    sd x29, 29*8(sp)
    sd x30, 30*8(sp)
    sd x31, 31*8(sp)
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    csrr t2, sscratch
    sd t2, 2*8(sp)          # 保存用户 sp
    ld t0, 34*8(sp)         # kernel_satp
    ld t1, 36*8(sp)         # trap_handler
    ld sp, 35*8(sp)         # kernel_sp（物理地址）
    csrw satp, t0
    sfence.vma
    mv a0, sp               # cx = kernel_sp = TrapContext 物理地址
    jr t1

__restore:
    # a0 = TrapContext 用户 VA, a1 = 用户 satp
    csrw satp, a1
    sfence.vma
    csrw sscratch, a0
    mv sp, a0
    ld t0, 32*8(sp)
    csrw sstatus, t0
    ld t1, 33*8(sp)
    csrw sepc, t1
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    ld x5, 5*8(sp)
    ld x6, 6*8(sp)
    ld x7, 7*8(sp)
    ld x8, 8*8(sp)
    ld x9, 9*8(sp)
    ld x10, 10*8(sp)
    ld x11, 11*8(sp)
    ld x12, 12*8(sp)
    ld x13, 13*8(sp)
    ld x14, 14*8(sp)
    ld x15, 15*8(sp)
    ld x16, 16*8(sp)
    ld x17, 17*8(sp)
    ld x18, 18*8(sp)
    ld x19, 19*8(sp)
    ld x20, 20*8(sp)
    ld x21, 21*8(sp)
    ld x22, 22*8(sp)
    ld x23, 23*8(sp)
    ld x24, 24*8(sp)
    ld x25, 25*8(sp)
    ld x26, 26*8(sp)
    ld x27, 27*8(sp)
    ld x28, 28*8(sp)
    ld x29, 29*8(sp)
    ld x30, 30*8(sp)
    ld x31, 31*8(sp)
    ld sp, 2*8(sp)
    sret
"#
);

global_asm!(
    r#"
    .section .text
    .globl kernel_trap
    .align 2
kernel_trap:
    addi sp, sp, -32
    csrr t0, scause
    csrr t1, stval
    csrr t2, sepc
    sd t0, 0(sp)
    sd t1, 8(sp)
    sd t2, 16(sp)
    mv a0, sp
    call kernel_trap_handler
"#
);

extern "C" {
    fn __restore(cx_va: usize, user_satp: usize) -> !;
    fn strampoline();
    fn kernel_trap();
}

#[no_mangle]
fn kernel_trap_handler(info: &[usize; 3]) -> ! {
    println!(
        "[KERNEL TRAP] scause={:#x} stval={:#x} sepc={:#x}",
        info[0], info[1], info[2]
    );
    panic!("kernel trap");
}

pub fn init() {
    set_kernel_stvec();
    crate::mm::AddressSpace::activate_kernel();
    println!("trap initialized, kernel page table activated");
}

fn set_kernel_stvec() {
    unsafe {
        asm!("csrw stvec, {}", in(reg) kernel_trap as usize);
    }
}

fn set_user_stvec() {
    unsafe {
        asm!("csrw stvec, {}", in(reg) TRAMPOLINE);
    }
}

/// 用户态 sstatus：SPP=0(U), SPIE=1, FS=11
pub fn user_sstatus() -> usize {
    let mut sstatus: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) sstatus);
    }
    sstatus &= !(1 << 8); // SPP = 0
    sstatus |= 1 << 5; // SPIE = 1
    sstatus |= 3 << 13; // FS = 11
    sstatus
}

/// trap 返回用户态（或首次进入用户态）
pub fn trap_return(cx: &TrapContext, user_satp: usize) -> ! {
    set_user_stvec();
    let restore_va = TRAMPOLINE + (__restore as usize - strampoline as usize);
    let cx_va = TRAP_CONTEXT + PAGE_SIZE - CTX_SIZE;
    debug_assert_eq!(cx as *const _ as usize, cx.kernel_sp);
    let f: extern "C" fn(usize, usize) -> ! = unsafe { core::mem::transmute(restore_va) };
    f(cx_va, user_satp)
}

/// trap 主处理函数（a0 = TrapContext 物理地址）
pub fn trap_handler_addr() -> usize {
    trap_handler as usize
}

#[no_mangle]
extern "C" fn trap_handler(cx: &mut TrapContext) -> ! {
    set_kernel_stvec();
    let scause: usize;
    let stval: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) scause);
        asm!("csrr {}, stval", out(reg) stval);
    }
    let interrupt = scause & (1 << 63) != 0;
    let code = scause & !(1 << 63);
    match (interrupt, code) {
        (false, 8) => {
            // 用户态 ecall
            cx.sepc += 4;
            let ret = crate::syscall::syscall(
                cx.x[17],
                [cx.x[10], cx.x[11], cx.x[12], cx.x[13], cx.x[14], cx.x[15]],
            );
            cx.x[10] = ret as usize;
        }
        (true, 5) => {
            // S 态定时器中断
            crate::timer::set_next_trigger();
            crate::task::schedule();
        }
        _ => {
            let task = crate::task::current_task();
            let pid = task.map(|t| t.pid).unwrap_or(0);
            println!(
                "[TRAP] pid={} scause={:#x} stval={:#x} sepc={:#x}",
                pid, scause, stval, cx.sepc
            );
            println!(
                "[TRAP] ra={:#x} sp={:#x} s0={:#x} a0={:#x} a1={:#x} a2={:#x}",
                cx.x[1], cx.x[2], cx.x[8], cx.x[10], cx.x[11], cx.x[12]
            );
            // 通过帧指针回溯调用栈
            if let Some(task) = crate::task::current_task() {
                let inner = task.inner.lock();
                let mut fp = cx.x[8];
                for _ in 0..8 {
                    if fp == 0 || fp % 8 != 0 {
                        break;
                    }
                    let prev = crate::mm::copy_in(&inner.space, fp - 16, &mut [0u8; 16])
                        .map(|_| {
                            let mut buf = [0u8; 16];
                            crate::mm::copy_in(&inner.space, fp - 16, &mut buf).ok();
                            (usize::from_ne_bytes(buf[0..8].try_into().unwrap()),
                             usize::from_ne_bytes(buf[8..16].try_into().unwrap()))
                        });
                    match prev {
                        Ok((prev_fp, ra)) => {
                            println!("[TRAP]   fp={:#x} ra={:#x}", fp, ra);
                            fp = prev_fp;
                        }
                        Err(_) => break,
                    }
                }
            }
            if interrupt {
                println!("unexpected interrupt, continue");
            } else {
                // 用户态异常：杀死进程
                crate::task::exit_current(128 + 4);
            }
        }
    }
    let task = crate::task::current_task().expect("no current task in trap return");
    let satp = task.user_satp();
    trap_return(cx, satp);
}
