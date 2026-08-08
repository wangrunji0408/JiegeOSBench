use core::arch::asm;

use crate::console;

pub const USER_STACK_TOP: usize = 0x8f00_0000;

#[repr(align(16))]
struct Stack([u8; 32768]);

static mut KERNEL_STACK: Stack = Stack([0; 32768]);

fn kernel_stack_top() -> usize {
    unsafe { (&raw mut KERNEL_STACK.0 as *mut u8).add(32768) as usize }
}

#[repr(C)]
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub sepc: usize,
    pub sstatus: usize,
    pub scause: usize,
    pub stval: usize,
}

impl TrapFrame {
    #[inline]
    pub fn arg(&self, n: usize) -> usize {
        self.regs[10 + n]
    }
    #[inline]
    pub fn set_ret(&mut self, value: isize) {
        self.regs[10] = value as usize;
    }
}

unsafe extern "C" {
    static mut trap_vector: u8;
}

#[inline]
pub fn time() -> u64 {
    let value;
    unsafe {
        asm!("rdtime {}", out(reg) value);
    }
    value
}

pub fn init() {
    unsafe {
        let vector = &raw const trap_vector as *const u8 as usize;
        asm!("csrw stvec, {}", in(reg) vector);
        asm!("csrw sscratch, {}", in(reg) kernel_stack_top());
        let mut sstatus: usize;
        asm!("csrr {}, sstatus", out(reg) sstatus);
        sstatus &= !(1 << 8);
        sstatus |= 1 << 5;
        asm!("csrw sstatus, {}", in(reg) sstatus);
    }
}

pub fn enter_user(entry: usize, stack: usize) -> ! {
    unsafe {
        let mut sstatus: usize;
        asm!("csrr {}, sstatus", out(reg) sstatus);
        sstatus &= !(1 << 8);
        sstatus |= 1 << 5;
        asm!("csrw sstatus, {}", in(reg) sstatus);
        asm!("csrw sepc, {}", in(reg) entry);
        asm!("mv sp, {0}; sret", in(reg) stack, options(noreturn));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_halt() -> ! {
    loop {
        unsafe {
            asm!("nop");
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let interrupt = (tf.scause >> (usize::BITS - 1)) != 0;
    let cause = tf.scause & (!(1usize << (usize::BITS - 1)));
    if interrupt {
        return;
    }
    if cause == 8 {
        crate::syscall::dispatch(tf);
        return;
    }
    console::write_str("\nLuna: user trap cause=");
    console::write_hex(cause);
    console::write_str(" stval=");
    console::write_hex(tf.stval);
    console::write_str(" sepc=");
    console::write_hex(tf.sepc);
    console::write_str("\n");
    tf.sepc = user_halt as *const () as usize;
}
