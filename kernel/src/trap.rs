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

// sstatus 位（注意：FS 在 bits 13-14，与 mstatus 同位置）
pub const SSTATUS_SUM: u64 = 1 << 18;
pub const SSTATUS_FS_INITIAL: u64 = 1 << 13;
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

#[repr(C, align(16))]
struct Aligned<const N: usize>([u8; N]);

static mut KERNEL_STACK: Aligned<{ 128 * 1024 }> = Aligned([0; 128 * 1024]);

global_asm!(
    r#"
.attribute arch, "rv64gc"
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
    fsd f0, 256(sp)
    fsd f1, 264(sp)
    fsd f2, 272(sp)
    fsd f3, 280(sp)
    fsd f4, 288(sp)
    fsd f5, 296(sp)
    fsd f6, 304(sp)
    fsd f7, 312(sp)
    fsd f8, 320(sp)
    fsd f9, 328(sp)
    fsd f10, 336(sp)
    fsd f11, 344(sp)
    fsd f12, 352(sp)
    fsd f13, 360(sp)
    fsd f14, 368(sp)
    fsd f15, 376(sp)
    fsd f16, 384(sp)
    fsd f17, 392(sp)
    fsd f18, 400(sp)
    fsd f19, 408(sp)
    fsd f20, 416(sp)
    fsd f21, 424(sp)
    fsd f22, 432(sp)
    fsd f23, 440(sp)
    fsd f24, 448(sp)
    fsd f25, 456(sp)
    fsd f26, 464(sp)
    fsd f27, 472(sp)
    fsd f28, 480(sp)
    fsd f29, 488(sp)
    fsd f30, 496(sp)
    fsd f31, 504(sp)
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

    # 调试打点：写 'T' 到 UART（此时所有寄存器已保存，t0/t1 可自由使用）
    li t0, 0x10000000
    li t1, 0x54
    sb t1, 0(t0)

    # 切到内核栈（trap 只来自用户态）
    ld sp, 552(sp)

    # a0 = trapframe 指针（sscratch 当前保存的就是它）
    csrr a0, sscratch
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

    fld f0, 256(a0)
    fld f1, 264(a0)
    fld f2, 272(a0)
    fld f3, 280(a0)
    fld f4, 288(a0)
    fld f5, 296(a0)
    fld f6, 304(a0)
    fld f7, 312(a0)
    fld f8, 320(a0)
    fld f9, 328(a0)
    fld f10, 336(a0)
    fld f11, 344(a0)
    fld f12, 352(a0)
    fld f13, 360(a0)
    fld f14, 368(a0)
    fld f15, 376(a0)
    fld f16, 384(a0)
    fld f17, 392(a0)
    fld f18, 400(a0)
    fld f19, 408(a0)
    fld f20, 416(a0)
    fld f21, 424(a0)
    fld f22, 432(a0)
    fld f23, 440(a0)
    fld f24, 448(a0)
    fld f25, 456(a0)
    fld f26, 464(a0)
    fld f27, 472(a0)
    fld f28, 480(a0)
    fld f29, 488(a0)
    fld f30, 496(a0)
    fld f31, 504(a0)

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

/// 最小用户态冒烟测试：一页代码（li a7,93; ecall）+ 一页栈
pub fn user_mode_smoke_test() {
    use crate::page::{PTE_A, PTE_D, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X};
    let root = crate::page::alloc_table().expect("smoke: alloc root");
    crate::page::map_kernel_regions(root);
    let code_va = 0x10_0000_0000usize;
    let code_pa = crate::pmm::alloc_page().expect("smoke: code page");
    crate::page::map_4k(root, code_va, code_pa, PTE_R | PTE_X | PTE_U | PTE_A | PTE_D | PTE_V);
    unsafe {
        let p = code_pa as *mut u32;
        p.write_volatile(0x05d00893); // li a7, 93 (exit_group)
        p.add(1).write_volatile(0xff410583); // ld a1, -12(sp) ← 测试栈 load
        p.add(2).write_volatile(0x00000073); // ecall
        p.add(3).write_volatile(0x00000073); // ecall（兜底）
    }
    let stack_top = 0x7_0000_0000usize; // 低地址栈（VPN[2]=28），排除高位 VA 问题
    let stack_pa = crate::pmm::alloc_page().expect("smoke: stack page");
    crate::page::map_4k(root, stack_top - 0x1000, stack_pa, PTE_R | PTE_W | PTE_U | PTE_A | PTE_D | PTE_V);
    let frame = trapframe_addr() as *mut TrapFrame;
    unsafe {
        (*frame).x = [0; 32];
        (*frame).f = [0; 32];
        (*frame).fcsr = 0;
        (*frame).sepc = code_va as u64;
        (*frame).x[2] = stack_top as u64;
        (*frame).sstatus = SSTATUS_SPIE | SSTATUS_SUM | SSTATUS_FS_INITIAL;
    }
    kprintln!("[smoke] entering user: code={:#x} stack={:#x}", code_va, stack_top);
    unsafe {
        crate::page::load_satp(root);
        core::arch::asm!("csrw sscratch, {}", in(reg) frame as usize);
        trap_return(frame)
    }
}

#[no_mangle]
extern "C" fn trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    let f = unsafe { &mut *frame };
    let cause = f.scause;
    kprintln!("[trap] scause={:#x} sepc={:#x} stval={:#x} a7={:#x} a0={:#x} a1={:#x} sp={:#x} s2={:#x}", cause, f.sepc, f.stval, f.x[17], f.x[10], f.x[11], f.x[2], f.x[18]);
    if cause_code == 13 || cause_code == 15 {
        // dump 初始栈区（sp 向上 0x100）
        let sp = f.x[2] as usize & !0xf;
        let mut dump = alloc::string::String::new();
        use core::fmt::Write;
        for i in 0..16 {
            let v = unsafe { ((sp + i * 8) as *const u64).read_volatile() };
            let _ = write!(dump, " {:016x}", v);
        }
        kprintln!("[trap] stack @ {:#x}:{}", sp, dump);
    }
    let cause_code = cause & 0xfff_ffff_ffff_ffff;
    if cause_code == 13 || cause_code == 15 || cause_code == 12 {
        let root = proc::current_page_table_root();
        let page = f.stval as usize & !0xfff;
        // 手动 walk 打印每一级
        let vpn2 = (page >> 30) & 0x1ff;
        let vpn1 = (page >> 21) & 0x1ff;
        let vpn0 = (page >> 12) & 0x1ff;
        let pte2 = unsafe { ((root + vpn2 * 8) as *const u64).read_volatile() };
        kprintln!("[trap]   root={:#x} vpn2={} pte2={:#x}", root, vpn2, pte2);
        if pte2 & 1 != 0 {
            let l1 = ((pte2 >> 10) as usize) << 12;
            let pte1 = unsafe { ((l1 + vpn1 * 8) as *const u64).read_volatile() };
            kprintln!("[trap]   l1={:#x} vpn1={} pte1={:#x}", l1, vpn1, pte1);
            if pte1 & 1 != 0 {
                let l0 = ((pte1 >> 10) as usize) << 12;
                let pte0 = unsafe { ((l0 + vpn0 * 8) as *const u64).read_volatile() };
                kprintln!("[trap]   l0={:#x} vpn0={} pte0={:#x}", l0, vpn0, pte0);
            }
        }
    }
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
            proc::die(crate::errno::SIGILL);
        }
        _ => {
            kprintln!(
                "\n[trap] unhandled exception scause={:#x} sepc={:#x} stval={:#x}",
                cause, f.sepc, f.stval
            );
            proc::die(crate::errno::SIGSEGV);
        }
    }
    frame
}

pub fn init() {
    unsafe {
        TRAPFRAME.kernel_sp = (KERNEL_STACK.0.as_ptr() as usize) + KERNEL_STACK.0.len();
        TRAPFRAME.sstatus = SSTATUS_SPIE | SSTATUS_SUM | SSTATUS_FS_INITIAL;
        core::arch::asm!("csrw stvec, {}", in(reg) trap_entry as usize);
        // 开启浮点（sstatus.FS = Initial, bits 13-14）
        let mut ss: u64;
        core::arch::asm!("csrr {}, sstatus", out(reg) ss);
        ss = (ss & !0x6000) | SSTATUS_FS_INITIAL;
        core::arch::asm!("csrw sstatus, {}", in(reg) ss);
    }
}

extern "C" {
    static trap_entry: u8;
}

pub fn trap_entry_addr() -> usize {
    unsafe { core::ptr::addr_of!(trap_entry) as usize }
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
