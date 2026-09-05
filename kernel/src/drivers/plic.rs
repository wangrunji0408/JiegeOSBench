//! SiFive PLIC (QEMU virt), S-mode context of hart 0.
const PLIC_BASE: usize = 0x0c00_0000;
const CONTEXT_S0: usize = 1; // hart 0 S-mode context

fn priority(irq: usize) -> *mut u32 {
    (PLIC_BASE + irq * 4) as *mut u32
}
fn enable_reg(ctx: usize, irq: usize) -> *mut u32 {
    (PLIC_BASE + 0x2000 + ctx * 0x80 + (irq / 32) * 4) as *mut u32
}
fn threshold(ctx: usize) -> *mut u32 {
    (PLIC_BASE + 0x20_0000 + ctx * 0x1000) as *mut u32
}
fn claim(ctx: usize) -> *mut u32 {
    (PLIC_BASE + 0x20_0004 + ctx * 0x1000) as *mut u32
}

pub fn init() {
    unsafe { threshold(CONTEXT_S0).write_volatile(0) };
}

pub fn enable(irq: usize) {
    unsafe {
        priority(irq).write_volatile(1);
        let r = enable_reg(CONTEXT_S0, irq);
        r.write_volatile(r.read_volatile() | (1 << (irq % 32)));
    }
}

pub fn handle_irq() {
    loop {
        let irq = unsafe { claim(CONTEXT_S0).read_volatile() } as usize;
        if irq == 0 {
            break;
        }
        if irq == crate::console::UART_IRQ {
            crate::console::handle_irq();
        } else if irq == super::virtio_net::irq() {
            super::virtio_net::handle_irq();
        } else {
            klog!("spurious irq {}", irq);
        }
        unsafe { claim(CONTEXT_S0).write_volatile(irq as u32) };
    }
}
