#![no_std]
#![no_main]
#![allow(static_mut_refs)]
extern crate alloc;
use core::{
    arch::{asm, global_asm},
    fmt::{self, Write},
    panic::PanicInfo,
};
mod elf;
mod fs;
mod memory;
mod net;
mod syscall;
global_asm!(include_str!("entry.S"));
#[global_allocator]
static HEAP: buddy_system_allocator::LockedHeap<32> = buddy_system_allocator::LockedHeap::empty();
#[repr(align(4096))]
struct HeapSpace([u8; 32 * 1024 * 1024]);
static mut HEAP_SPACE: HeapSpace = HeapSpace([0; 32 * 1024 * 1024]);
pub struct Console;
impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            putchar(b);
        }
        Ok(())
    }
}
pub fn putchar(b: u8) {
    unsafe {
        asm!("ecall",in("a7")1usize,in("a0")b as usize,lateout("a1")_,options(nostack));
    }
}
#[macro_export]
macro_rules! print { ($($arg:tt)*) => {{ use core::fmt::Write; let _=write!($crate::Console,$($arg)*); }} }
#[macro_export]
macro_rules! println { ($($arg:tt)*) => { $crate::print!("{}\n",format_args!($($arg)*)) } }
#[panic_handler]
fn panic(p: &PanicInfo) -> ! {
    println!("PANIC: {}", p);
    shutdown()
}
pub fn shutdown() -> ! {
    unsafe {
        asm!("ecall",in("a7")0x53525354usize,in("a6")0usize,in("a0")0usize,in("a1")0usize);
    }
    loop {
        unsafe { asm!("wfi") }
    }
}
pub fn ticks() -> u64 {
    let t;
    unsafe { asm!("rdtime {}",out(reg)t) }
    t
}
pub fn millis() -> i64 {
    (ticks() / 10000) as i64
}
#[repr(C)]
pub struct Context {
    pub x: [usize; 32],
    pub pc: usize,
    pub status: usize,
}
extern "C" {
    fn enter_user(c: *mut Context) -> !;
    fn kernel_trap();
}
#[no_mangle]
fn kmain() -> ! {
    unsafe {
        asm!("csrw stvec, {}",in(reg)kernel_trap as *const () as usize);
        HEAP.lock().init(
            core::ptr::addr_of_mut!(HEAP_SPACE.0) as usize,
            32 * 1024 * 1024,
        );
        println!("\n智能杰哥 iJiege — Rust RISC-V Linux ABI kernel");
        memory::init();
        fs::init();
        net::init();
        let (pc, sp) = elf::load_program(
            "/usr/sbin/nginx",
            &["nginx", "-p", "/", "-c", "/etc/nginx/ijiege.conf"],
        );
        println!(
            "[exec] unmodified Alpine nginx ELF entry={:#x} sp={:#x}",
            pc, sp
        );
        let mut ctx = Context {
            x: [0; 32],
            pc,
            status: (1 << 18) | (3 << 13),
        };
        ctx.x[2] = sp;
        enter_user(&mut ctx)
    }
}
#[no_mangle]
fn kernel_fault(c: usize, v: usize, p: usize) -> ! {
    panic!("kernel exception cause={} address={:#x} pc={:#x}", c, v, p)
}
#[no_mangle]
fn rust_trap(c: &mut Context) {
    let cause: usize;
    let val: usize;
    unsafe {
        asm!("csrr {}, scause",out(reg)cause);
        asm!("csrr {}, stval",out(reg)val);
    }
    if cause == 8 {
        c.pc += 4;
        c.x[10] = syscall::dispatch(
            c.x[17],
            [c.x[10], c.x[11], c.x[12], c.x[13], c.x[14], c.x[15]],
        ) as usize;
    } else {
        println!(
            "[fault] user cause={} addr={:#x} pc={:#x} ra={:#x} sp={:#x}",
            cause, val, c.pc, c.x[1], c.x[2]
        );
        println!("regs {:x?}", c.x);
        shutdown();
    }
}
