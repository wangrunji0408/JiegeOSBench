//! PLIC (platform-level interrupt controller) on the QEMU virt machine.

use crate::mm::address::dev_addr;
use crate::sync::SpinLock;

const PLIC_BASE: usize = 0x0c00_0000;
const PRIORITY: usize = 0x0000_0000;
const ENABLE: usize = 0x0000_2000;
const THRESHOLD: usize = 0x0020_0000;
const CLAIM: usize = 0x0020_0004;

/// S-mode context for hart 0.
const CONTEXT: usize = 1;

const MAX_IRQ: usize = 64;

struct Plic {
    handlers: [Option<fn(usize)>; MAX_IRQ],
}

static PLIC: SpinLock<Plic> = SpinLock::new(Plic {
    handlers: [None; MAX_IRQ],
});

#[inline]
fn r32(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(dev_addr(PLIC_BASE + off) as *const u32) }
}

#[inline]
fn w32(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile(dev_addr(PLIC_BASE + off) as *mut u32, v) }
}

pub fn init() {
    // lowest priority threshold for this context
    w32(THRESHOLD + 4 * CONTEXT, 0);
}

pub fn register(irq: usize, f: fn(usize)) {
    PLIC.lock().handlers[irq] = Some(f);
    w32(PRIORITY + 4 * irq, 1);
    w32(ENABLE + 4 * CONTEXT, 1u32 << irq);
}

/// Claim and dispatch all pending interrupts.
pub fn handle() {
    loop {
        let irq = r32(CLAIM) as usize;
        if irq == 0 {
            break;
        }
        let h = PLIC.lock().handlers.get(irq).copied().flatten();
        if let Some(f) = h {
            f(irq);
        }
        w32(CLAIM, irq as u32);
    }
}
