/// VirtIO块设备驱动
/// 用于加载initramfs文件系统

use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::mm::alloc_frame;
use crate::config::PAGE_SIZE;

// VirtIO MMIO寄存器偏移
const STATUS: usize = 0x070;
const DEVICE_FEATURES: usize = 0x010;
const DRIVER_FEATURES: usize = 0x020;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;

// VirtIO状态
const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const VIRTQ_SIZE: usize = 16;

#[repr(C, align(16))]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

#[repr(C, align(2))]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; VIRTQ_SIZE],
}

#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C, align(4))]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; VIRTQ_SIZE],
}

// VirtIO块请求头
#[repr(C)]
struct BlkReqHeader {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

const VIRTIO_BLK_T_IN: u32 = 0;  // 读
const VIRTIO_BLK_T_OUT: u32 = 1; // 写

struct BlockDevice {
    base: usize,
    desc: &'static mut [VirtqDesc; VIRTQ_SIZE],
    avail: &'static mut VirtqAvail,
    used: &'static mut VirtqUsed,
    desc_used: [bool; VIRTQ_SIZE],
    last_used: u16,
}

static BLOCK_DEVICE: Mutex<Option<BlockDevice>> = Mutex::new(None);

fn read_reg(base: usize, off: usize) -> u32 {
    unsafe { ((base + off) as *const u32).read_volatile() }
}

fn write_reg(base: usize, off: usize, val: u32) {
    unsafe { ((base + off) as *mut u32).write_volatile(val) }
}

pub fn init_block_device(base: usize) {
    // 重置设备
    write_reg(base, STATUS, 0);
    // ACKNOWLEDGE + DRIVER
    write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

    // 协商特性（我们不需要特殊特性）
    let features = read_reg(base, DEVICE_FEATURES);
    write_reg(base, DRIVER_FEATURES, features & 0x0); // 不接受任何特性
    write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);

    // 检查FEATURES_OK
    let status = read_reg(base, STATUS);
    if status & STATUS_FEATURES_OK == 0 {
        println!("[block] Features negotiation failed");
        return;
    }

    // 分配virtqueue
    write_reg(base, QUEUE_SEL, 0);
    let num_max = read_reg(base, QUEUE_NUM_MAX) as usize;
    let num = VIRTQ_SIZE.min(num_max);
    write_reg(base, QUEUE_NUM, num as u32);

    // 分配描述符表、可用环、已用环
    let desc_frame = alloc_frame().expect("no memory for virtq desc");
    let avail_frame = alloc_frame().expect("no memory for virtq avail");
    let used_frame = alloc_frame().expect("no memory for virtq used");

    let desc_pa = desc_frame.0.addr();
    let avail_pa = avail_frame.0.addr();
    let used_pa = used_frame.0.addr();

    write_reg(base, QUEUE_DESC_LOW, desc_pa as u32);
    write_reg(base, QUEUE_DESC_HIGH, (desc_pa >> 32) as u32);
    write_reg(base, QUEUE_DRIVER_LOW, avail_pa as u32);
    write_reg(base, QUEUE_DRIVER_HIGH, (avail_pa >> 32) as u32);
    write_reg(base, QUEUE_DEVICE_LOW, used_pa as u32);
    write_reg(base, QUEUE_DEVICE_HIGH, (used_pa >> 32) as u32);
    write_reg(base, QUEUE_READY, 1);

    // 设备就绪
    write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);

    let desc_va = crate::utils::phys_to_virt(desc_pa);
    let avail_va = crate::utils::phys_to_virt(avail_pa);
    let used_va = crate::utils::phys_to_virt(used_pa);

    // 保持frames存活（内存泄漏，但这是内核，无所谓）
    core::mem::forget(desc_frame);
    core::mem::forget(avail_frame);
    core::mem::forget(used_frame);

    let dev = BlockDevice {
        base,
        desc: unsafe { &mut *(desc_va as *mut [VirtqDesc; VIRTQ_SIZE]) },
        avail: unsafe { &mut *(avail_va as *mut VirtqAvail) },
        used: unsafe { &mut *(used_va as *mut VirtqUsed) },
        desc_used: [false; VIRTQ_SIZE],
        last_used: 0,
    };

    *BLOCK_DEVICE.lock() = Some(dev);
    println!("[block] Block device initialized at {:#x}", base);
}

pub fn handle_irq(base: usize) {
    // 不需要特别处理，轮询即可
}

/// 同步读取块设备扇区
pub fn read_sector(sector: u64, buf: &mut [u8; 512]) {
    let mut guard = BLOCK_DEVICE.lock();
    let dev = guard.as_mut().expect("no block device");

    // 找空闲描述符（需要3个：头、数据、状态）
    let mut free_descs = [0usize; 3];
    let mut found = 0;
    for i in 0..VIRTQ_SIZE {
        if !dev.desc_used[i] {
            free_descs[found] = i;
            found += 1;
            if found == 3 { break; }
        }
    }
    assert_eq!(found, 3, "no free descriptors");

    // 分配请求头
    let header_frame = alloc_frame().expect("no memory for blk header");
    let header_pa = header_frame.0.addr();
    let header_va = crate::utils::phys_to_virt(header_pa);
    let header = unsafe { &mut *(header_va as *mut BlkReqHeader) };
    header.req_type = VIRTIO_BLK_T_IN;
    header.reserved = 0;
    header.sector = sector;

    // 分配数据缓冲区
    let data_frame = alloc_frame().expect("no memory for blk data");
    let data_pa = data_frame.0.addr();

    // 分配状态字节
    let status_frame = alloc_frame().expect("no memory for blk status");
    let status_pa = status_frame.0.addr();

    // 设置描述符
    let (d0, d1, d2) = (free_descs[0], free_descs[1], free_descs[2]);

    dev.desc[d0] = VirtqDesc {
        addr: header_pa as u64,
        len: core::mem::size_of::<BlkReqHeader>() as u32,
        flags: VIRTQ_DESC_F_NEXT,
        next: d1 as u16,
    };
    dev.desc[d1] = VirtqDesc {
        addr: data_pa as u64,
        len: 512,
        flags: VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
        next: d2 as u16,
    };
    dev.desc[d2] = VirtqDesc {
        addr: status_pa as u64,
        len: 1,
        flags: VIRTQ_DESC_F_WRITE,
        next: 0,
    };

    dev.desc_used[d0] = true;
    dev.desc_used[d1] = true;
    dev.desc_used[d2] = true;

    // 添加到可用环
    let avail_idx = dev.avail.idx as usize % VIRTQ_SIZE;
    dev.avail.ring[avail_idx] = d0 as u16;
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    dev.avail.idx = dev.avail.idx.wrapping_add(1);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    // 通知设备
    write_reg(dev.base, QUEUE_NOTIFY, 0);

    // 等待完成（忙等待）
    loop {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        if dev.used.idx != dev.last_used {
            break;
        }
    }
    dev.last_used = dev.used.idx;

    // 释放描述符
    dev.desc_used[d0] = false;
    dev.desc_used[d1] = false;
    dev.desc_used[d2] = false;

    // 复制数据
    let data_va = crate::utils::phys_to_virt(data_pa);
    buf.copy_from_slice(unsafe { core::slice::from_raw_parts(data_va as *const u8, 512) });

    // 检查状态
    let status_va = crate::utils::phys_to_virt(status_pa);
    let status = unsafe { *(status_va as *const u8) };
    if status != 0 {
        panic!("block read failed: status={}", status);
    }
}

/// 检查是否有块设备
pub fn has_block_device() -> bool {
    BLOCK_DEVICE.lock().is_some()
}
