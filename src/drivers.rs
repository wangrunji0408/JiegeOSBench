pub mod virtio_net;

const PLIC_BASE: usize = 0x0c00_0000;
const PLIC_SUPERVISOR_CONTEXT: usize = 1;

fn plic_write(offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((PLIC_BASE + offset) as *mut u32, value) }
}

fn plic_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((PLIC_BASE + offset) as *const u32) }
}

pub fn init() {
    for irq in 1..=8usize {
        plic_write(irq * 4, 1);
    }
    let enable = 0x2000 + PLIC_SUPERVISOR_CONTEXT * 0x80;
    plic_write(enable, plic_read(enable) | 0xff);
    plic_write(0x20_0000 + PLIC_SUPERVISOR_CONTEXT * 0x1000, 0);
    crate::net::init();
}

pub fn timer_interrupt() {
    crate::net::poll();
}

pub fn external_interrupt() {
    let claim = 0x20_0004 + PLIC_SUPERVISOR_CONTEXT * 0x1000;
    let irq = plic_read(claim);
    if irq != 0 {
        if (1..=8).contains(&irq) {
            crate::net::interrupt();
        }
        plic_write(claim, irq);
    }
}
