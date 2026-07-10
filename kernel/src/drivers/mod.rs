mod virtio;
pub mod block;
pub mod net;

use alloc::sync::Arc;
use spin::Mutex;

use crate::config::*;

pub fn init(dtb_pa: usize) {
    // 探测VirtIO设备
    virtio::init_devices(dtb_pa);
}

pub fn handle_external_interrupt() {
    // 处理PLIC外部中断
    let plic_base = crate::utils::phys_to_virt(PLIC_BASE);
    // 读取PLIC完成寄存器（核0，S模式）
    let claim_reg = (plic_base + 0x201004) as *mut u32;
    let irq = unsafe { claim_reg.read_volatile() };
    if irq == 0 { return; }

    // 处理中断
    match irq {
        1 => {
            // UART中断（忽略输入）
        }
        _ => {
            // VirtIO中断
            virtio::handle_irq(irq);
        }
    }

    // 完成中断
    unsafe { claim_reg.write_volatile(irq); }
}
