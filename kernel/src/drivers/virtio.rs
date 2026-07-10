/// VirtIO设备初始化和管理
/// QEMU virt机器：virtio-mmio设备在0x10001000 - 0x10008000
/// 每个设备占0x1000字节

use crate::config::*;
use crate::utils::phys_to_virt;

// VirtIO MMIO寄存器偏移
const VIRTIO_MMIO_MAGIC_VALUE: usize = 0x000;
const VIRTIO_MMIO_VERSION: usize = 0x004;
const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
const VIRTIO_MMIO_VENDOR_ID: usize = 0x00c;
const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
const VIRTIO_MMIO_QUEUE_READY: usize = 0x044;
const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
const VIRTIO_MMIO_INTERRUPT_STATUS: usize = 0x060;
const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
const VIRTIO_MMIO_STATUS: usize = 0x070;
const VIRTIO_MMIO_QUEUE_DESC_LOW: usize = 0x080;
const VIRTIO_MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const VIRTIO_MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const VIRTIO_MMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
const VIRTIO_MMIO_QUEUE_DEVICE_LOW: usize = 0x0a0;
const VIRTIO_MMIO_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const VIRTIO_MMIO_CONFIG_GENERATION: usize = 0x0fc;
const VIRTIO_MMIO_CONFIG: usize = 0x100;

const VIRTIO_MAGIC: u32 = 0x74726976;

// VirtIO设备类型
const VIRTIO_ID_BLOCK: u32 = 2;
const VIRTIO_ID_NET: u32 = 1;

pub fn init_devices(dtb_pa: usize) {
    // 扫描VirtIO MMIO地址范围
    for i in 0..VIRTIO_COUNT {
        let base_pa = VIRTIO_BASE + i * VIRTIO_SIZE;
        let base_va = phys_to_virt(base_pa);

        let magic = read_reg(base_va, VIRTIO_MMIO_MAGIC_VALUE);
        if magic != VIRTIO_MAGIC {
            continue;
        }

        let device_id = read_reg(base_va, VIRTIO_MMIO_DEVICE_ID);
        let version = read_reg(base_va, VIRTIO_MMIO_VERSION);

        match device_id {
            VIRTIO_ID_BLOCK => {
                println!("[virtio] Found block device at {:#x} (version {})", base_pa, version);
                super::block::init_block_device(base_va);
            }
            VIRTIO_ID_NET => {
                println!("[virtio] Found net device at {:#x} (version {})", base_pa, version);
                super::net::init_net_device(base_va, version);
            }
            0 => {} // 空设备
            id => {
                println!("[virtio] Unknown device id={} at {:#x}", id, base_pa);
            }
        }
    }
}

pub fn handle_irq(irq: u32) {
    // 遍历所有设备，找到产生中断的设备
    for i in 0..VIRTIO_COUNT {
        let base_pa = VIRTIO_BASE + i * VIRTIO_SIZE;
        let base_va = phys_to_virt(base_pa);

        let magic = read_reg(base_va, VIRTIO_MMIO_MAGIC_VALUE);
        if magic != VIRTIO_MAGIC { continue; }

        let int_status = read_reg(base_va, VIRTIO_MMIO_INTERRUPT_STATUS);
        if int_status != 0 {
            write_reg(base_va, VIRTIO_MMIO_INTERRUPT_ACK, int_status);
            let device_id = read_reg(base_va, VIRTIO_MMIO_DEVICE_ID);
            match device_id {
                VIRTIO_ID_NET => {
                    super::net::handle_irq(base_va);
                }
                VIRTIO_ID_BLOCK => {
                    super::block::handle_irq(base_va);
                }
                _ => {}
            }
        }
    }
}

pub fn read_reg(base: usize, offset: usize) -> u32 {
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

pub fn write_reg(base: usize, offset: usize, val: u32) {
    unsafe { ((base + offset) as *mut u32).write_volatile(val) }
}
