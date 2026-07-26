//! The SiFive PLIC, as implemented by the QEMU `virt` machine.

use alloc::vec::Vec;
use spin::Mutex;

/// PLIC base address in the QEMU `virt` machine.
const PLIC_BASE: usize = 0x0c00_0000;
/// Interrupt priority registers, one 32-bit word per source.
const PRIORITY_OFFSET: usize = 0x0000;
/// Per-context enable bitmaps.
const ENABLE_OFFSET: usize = 0x2000;
/// Per-context priority threshold and claim/complete registers.
const CONTEXT_OFFSET: usize = 0x20_0000;

/// Hart 0's supervisor-mode context. In the `virt` machine, context 0 is hart 0
/// machine mode and context 1 is hart 0 supervisor mode.
const S_CONTEXT: usize = 1;

#[inline]
fn write_reg(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((PLIC_BASE + offset) as *mut u32, value) };
}

#[inline]
fn read_reg(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((PLIC_BASE + offset) as *const u32) }
}

/// Registered interrupt handlers, keyed by IRQ number.
type Handler = fn();
static HANDLERS: Mutex<Vec<(u32, Handler)>> = Mutex::new(Vec::new());

pub fn init() {
    // Accept every priority level.
    write_reg(CONTEXT_OFFSET + S_CONTEXT * 0x1000, 0);
}

/// Enable an IRQ and register its handler.
pub fn register(irq: u32, handler: Handler) {
    // Priority must be non-zero to be delivered at all.
    write_reg(PRIORITY_OFFSET + irq as usize * 4, 1);

    // Set the enable bit for our context.
    let enable_addr = ENABLE_OFFSET + S_CONTEXT * 0x80 + (irq as usize / 32) * 4;
    let current = read_reg(enable_addr);
    write_reg(enable_addr, current | (1 << (irq % 32)));

    HANDLERS.lock().push((irq, handler));
    crate::info!("plic: enabled irq {}", irq);
}

/// Claim and dispatch pending interrupts.
pub fn handle_interrupt() {
    let claim_addr = CONTEXT_OFFSET + S_CONTEXT * 0x1000 + 4;
    loop {
        let irq = read_reg(claim_addr);
        if irq == 0 {
            return;
        }
        let handler = HANDLERS
            .lock()
            .iter()
            .find(|(i, _)| *i == irq)
            .map(|(_, h)| *h);
        match handler {
            Some(h) => h(),
            None => crate::warn!("plic: unhandled irq {}", irq),
        }
        // Complete the interrupt.
        write_reg(claim_addr, irq);
    }
}
