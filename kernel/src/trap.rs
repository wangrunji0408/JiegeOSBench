//! Trap 上下文：用户态 <-> 内核态切换
//!
//! 约定：
//! - sscratch 常年保存 TRAPFRAME 地址（用户态运行时）
//! - trap 只会来自用户态（内核态运行时全局关中断，等待用 wfi+sip 轮询）
//! - 用户页表内恒等映射了全部内核区域（VA==PA），trap 后无需切换 satp

use crate::syscall;
use crate::{kprintln, proc};
use core::arch::global_asm;

#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 32],   // 0    通用寄存器, x[2]=sp
    pub f: [u64; 32],   // 256  浮点寄存器
    pub fcsr: u64,      // 512
    pub sepc: u64,      // 520
    pub sstatus: u64,   // 528
    pub scause: u64,    // 536
    pub stval: u64,     // 544
    pub kernel_sp: u64, // 552  内核栈顶
}

// sstatus 位
pub const SSTATUS_SUM: u64 = 1 << 18;
pub const SSTATUS_FS_INITIAL: u64 = 1 << 33;
pub const SSTATUS_SPIE: u64 = 1 << 5;

pub const TRAPFRAME_ADDR: usize = core::ptr::addr_of!(TRAPFRAME) as usize;

static mut TRAPFRAME: TrapFrame = TrapFrame {
    x: [0; 32],
    f: [0; 32],
    fcsr: 0,
    sepc: 0,
    sstatus: 0,
    scause: 0,
    stval: 0,
    kernel_sp: 0,
};

#[repr(align(16))]
static mut KERNEL_STACK: [u8; 128 * 1024] = [0; 128 * 1024];

global_asm!(
    r#"
.altmacro
.macro SAVE_FP base
    .set n, 0
    .rept 32
        fsd f%n, (256 + 8*n)(\base)
        .set n, n+1
    .endr
.endm
.macro LOAD_FP base
    .set n, 0
    .rept 32
        fld f%n, (256 + 8*n)(\base)
        .set n, n+1
    .endr
.endm

.section .text
.align 4
.globl trap_entry
trap_entry:
    # sp <-> sscratch: sp = trapframe, sscratch = 旧 sp
    csrrw sp, sscratch, sp

    # 保存通用寄存器（除 x0/sp；sp 稍后）
    sd ra, 8(sp)
    sd gp, 24(sp)
    sd tp, 32(sp)
    sd t0, 40(sp)
    sd t1, 48(sp)
    sd t2, 56(sp)
    sd s0, 64(sp)
    sd s1, 72(sp)
    sd a0, 80(sp)
    sd a1, 88(sp)
    sd a2, 96(sp)
    sd a3, 104(sp)
    sd a4, 112(sp)
    sd a5, 120(sp)
    sd a6, 128(sp)
    sd a7, 136(sp)
    sd s2, 144(sp)
    sd s3, 152(sp)
    sd s4, 160(sp)
    sd s5, 168(sp)
    sd s6, 176(sp)
    sd s7, 184(sp)
    sd s8, 192(sp)
    sd s9, 200(sp)
    sd s10, 208(sp)
    sd s11, 216(sp)
    sd t3, 224(sp)
    sd t4, 232(sp)
    sd t5, 240(sp)
    sd t6, 248(sp)

    # 保存旧 sp
    csrr t0, sscratch
    sd t0, 16(sp)

    # 保存浮点
    SAVE_FP sp
    csrr t0, fcsr
    sd t0, 512(sp)

    # 保存 CSR
    csrr t0, sepc
    sd t0, 520(sp)
    csrr t0, sstatus
    sd t0, 528(sp)
    csrr t0, scause
    sd t0, 536(sp)
    csrr t0, stval
    sd t0, 544(sp)

    # sscratch 重置为 trapframe 地址（下次 trap 用）
    csrw sscratch, sp

    # 切到内核栈（trap 只来自用户态）
    ld sp, 552(sp)

    mv a0, sp
    call trap_handler
    # trap_handler 返回 trapframe 指针, 直接落入 trap_return

.globl trap_return
trap_return:
    # a0 = trapframe 指针
    ld t0, 520(a0)
    csrw sepc, t0
    ld t0, 528(a0)
    csrw sstatus, t0
    ld t0, 536(a0)
    csrw scause, t0
    ld t0, 544(a0)
    csrw stval, t0
    ld t0, 512(a0)
    csrw fcsr, t0

    LOAD_FP a0

    # 恢复通用寄存器（t0/t1/sp/a0 最后处理）
    ld ra, 8(a0)
    ld gp, 24(a0)
    ld tp, 32(a0)
    ld t2, 56(a0)
    ld s0, 64(a0)
    ld s1, 72(a0)
    ld a1, 88(a0)
    ld a2, 96(a0)
    ld a3, 104(a0)
    ld a4, 112(a0)
    ld a5, 120(a0)
    ld a6, 128(a0)
    ld a7, 136(a0)
    ld s2, 144(a0)
    ld s3, 152(a0)
    ld s4, 160(a0)
    ld s5, 168(a0)
    ld s6, 176(a0)
    ld s7, 184(a0)
    ld s8, 192(a0)
    ld s9, 200(a0)
    ld s10, 208(a0)
    ld s11, 216(a0)
    ld t3, 224(a0)
    ld t4, 232(a0)
    ld t5, 240(a0)
    ld t6, 248(a0)

    # sp
    ld sp, 16(a0)

    # 最后恢复 t0/t1/a0
    mv t1, a0
    ld t0, 40(t1)
    ld a0, 80(t1)
    ld t1, 48(t1)

    sret
"#
);

extern "C" {
    fn trap_return(frame: *mut TrapFrame) -> !;
}

/// 首次进入用户态（proc::spawn 设置好 TRAPFRAME 后调用）
pub fn enter_user(frame: *mut TrapFrame) -> ! {
    unsafe {
        // 装载用户页表
        let root = proc::current_page_table_root();
        crate::page::load_satp(root);
        // sscratch 指向 trapframe
        core::arch::asm!("csrw sscratch, {}", in(reg) frame as usize);
        trap_return(frame)
    }
}

#[no_mangle]
extern "C" fn trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    let f = unsafe { &mut *frame };
    let cause = f.scause;
    let interrupt = cause >> 63 == 1;
    let code = cause & 0xfff_ffff_ffff_ffff;

    if interrupt {
        kprintln!("\n[trap] unexpected interrupt scause={:#x} sepc={:#x}", cause, f.sepc);
        // 内核态不应发生 trap 中断（SIE=0）；万一发生只记录
        return frame;
    }

    match code {
        8 => {
            // ecall from U-mode
            let nr = f.x[17] as usize; // a7
            let ret = syscall::dispatch(f, nr);
            f.x[10] = ret as u64; // a0
            f.sepc += 4;
        }
        12 | 13 | 15 => {
            // 指令/读/写 page fault
            let va = f.stval as usize;
            match proc::handle_page_fault(va, code) {
                Ok(()) => {}
                Err(e) => {
                    kprintln!(
                        "\n[segfault] va={:#x} cause={} ({} fault) sepc={:#x}",
                        va,
                        code,
                        if code == 12 { "instr" } else if code == 13 { "load" } else { "store" },
                        f.sepc
                    );
                    proc::die(e);
                }
            }
        }
        2 => {
            kprintln!("\n[illegal instruction] sepc={:#x} stval={:#x}", f.sepc, f.stval);
            proc::die(crate::errno::Errno::Sigill);
        }
        _ => {
            kprintln!(
                "\n[trap] unhandled exception scause={:#x} sepc={:#x} stval={:#x}",
                cause, f.sepc, f.stval
            );
            proc::die(crate::errno::Errno::Sigsegv);
        }
    }
    frame
}

pub fn init() {
    unsafe {
        TRAPFRAME.kernel_sp = (KERNEL_STACK.as_ptr() as usize) + KERNEL_STACK.len();
        TRAPFRAME.sstatus = SSTATUS_SPIE | SSTATUS_SUM | SSTATUS_FS_INITIAL;
        core::arch::asm!("csrw stvec, {}", in(reg) trap_entry as usize);
        // 开启浮点（sstatus.FS = Initial）
        let mut ss: u64;
        core::arch::asm!("csrr {}, sstatus", out(reg) ss);
        ss = (ss & !0x6000_0000) | SSTATUS_FS_INITIAL;
        core::arch::asm!("csrw sstatus, {}", in(reg) ss);
    }
}

extern "C" {
    static trap_entry: u8;
}

/// 读 mtime（QEMU virt: 10MHz）
#[inline]
pub fn time_ticks() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("rdtime {}", out(reg) v);
    }
    v
}

pub const TIMEBASE_FREQ: u64 = 10_000_000;

#[inline]
pub fn now_ms() -> u64 {
    time_ticks() / (TIMEBASE_FREQ / 1000)
}
