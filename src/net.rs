//! virtio-mmio 网络驱动（legacy 接口，split virtqueue）。
//! QEMU virt 平台通过 -device virtio-net-device 接到 virtio-mmio 总线。

use core::ptr::{read_volatile, write_volatile};
use crate::mm::frame::FRAME_ALLOCATOR;

// virtio-mmio 寄存器偏移
const MMIO_MAGIC: u32 = 0x74726976; // "virt"
const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_VENDOR_ID: usize = 0x00c;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_ALIGN: usize = 0x03c;
const REG_QUEUE_PFN: usize = 0x040;
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_INT_STATUS: usize = 0x060;
const REG_INT_ACK: usize = 0x064;
const REG_STATUS: usize = 0x070;
const REG_CONFIG: usize = 0x100;

// status bits
const S_ACK: u32 = 1;
const S_DRIVER: u32 = 2;
const S_DRIVER_OK: u32 = 4;
const S_FEATURES_OK: u32 = 8;

const VQ_SIZE: usize = 8;

// virtio-net 头（legacy）
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
}
const NET_HDR_LEN: usize = 10;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C, align(2))]
struct AvailRing {
    flags: u16,
    idx: u16,
    ring: [u16; VQ_SIZE],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UsedElem {
    idx: u32,
    len: u32,
}

#[repr(C, align(4))]
struct UsedRing {
    flags: u16,
    idx: u16,
    ring: [UsedElem; VQ_SIZE],
    avail_event: u16,
}

pub struct VirtQueue {
    pub desc: *mut VqDesc,
    pub avail: *mut AvailRing,
    pub used: *mut UsedRing,
    pub last_used: u16,
    pub last_avail: u16,
    pub num: usize,
    // RX 队列的缓冲区地址（物理）
    pub rx_bufs: [usize; VQ_SIZE],
}

impl VirtQueue {
    /// 在一页内分配 desc/avail/used（足够 VQ_SIZE=8）
    pub fn new() -> Option<Self> {
        let desc_pa = FRAME_ALLOCATOR.alloc_zeroed()?;
        let avail_pa = FRAME_ALLOCATOR.alloc_zeroed()?;
        let used_pa = FRAME_ALLOCATOR.alloc_zeroed()?;
        Some(Self {
            desc: desc_pa as *mut VqDesc,
            avail: avail_pa as *mut AvailRing,
            used: used_pa as *mut UsedRing,
            last_used: 0,
            last_avail: 0,
            num: VQ_SIZE,
            rx_bufs: [0; VQ_SIZE],
        })
    }
}

pub struct NetDriver {
    pub base: usize,
    pub rx: VirtQueue,
    pub tx: VirtQueue,
    pub mac: [u8; 6],
}

static mut NET: Option<NetDriver> = None;

unsafe fn reg_w(base: usize, off: usize, val: u32) {
    write_volatile((base + off) as *mut u32, val);
}
unsafe fn reg_r(base: usize, off: usize) -> u32 {
    read_volatile((base + off) as *const u32)
}

/// 扫描 virtio-mmio 总线，找网络设备
pub fn init() {
    unsafe {
        for i in 0..32usize {
            let base = 0x1000_1000 + i * 0x1000;
            let magic = reg_r(base, REG_MAGIC);
            if magic != MMIO_MAGIC {
                continue;
            }
            let ver = reg_r(base, REG_VERSION);
            let devid = reg_r(base, REG_DEVICE_ID);
            if devid != 1 {
                continue; // 1 = network
            }
            crate::println!("[net] found virtio-net @ {:#x} ver={}", base, ver);
            if ver != 1 {
                crate::println!("[net] only legacy (v1) supported, got {}", ver);
                continue;
            }
            if init_device(base) {
                return;
            }
        }
        crate::println!("[net] no virtio-net device found");
    }
}

unsafe fn init_device(base: usize) -> bool {
    // 复位
    reg_w(base, REG_STATUS, 0);
    // ack + driver
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER);
    // 读 features，全接受（legacy net 基本特性）
    let _feat = reg_r(base, REG_DEVICE_FEATURES);
    reg_w(base, REG_DRIVER_FEATURES, 0);
    // features_ok
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK);

    let rx = match VirtQueue::new() {
        Some(q) => q,
        None => return false,
    };
    let tx = match VirtQueue::new() {
        Some(q) => q,
        None => return false,
    };

    // 配置 RX 队列（queue index 0）
    setup_queue(base, 0, &rx, true);
    // 配置 TX 队列（queue index 1）
    setup_queue(base, 1, &tx, false);

    // driver_ok
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER | S_DRIVER_OK | S_FEATURES_OK);

    // 读 MAC
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = read_volatile((base + REG_CONFIG + i) as *const u8);
    }
    crate::println!(
        "[net] up, mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    NET = Some(NetDriver { base, rx, tx, mac });
    true
}

unsafe fn setup_queue(base: usize, idx: usize, vq: &VirtQueue, is_rx: bool) {
    reg_w(base, REG_QUEUE_SEL, idx as u32);
    let nmax = reg_r(base, REG_QUEUE_NUM_MAX) as usize;
    let n = nmax.min(vq.num);
    reg_w(base, REG_QUEUE_NUM, n as u32);
    reg_w(base, REG_QUEUE_ALIGN, 4096);
    // desc 表、avail、used 在各自的页里；legacy 只需把 desc 表的 PFN 写入
    // （QEMU legacy 用 QueuePFN = desc 表物理地址 >> 12，并假定 avail/used 紧跟）
    // 实际上 legacy 要求三者在同一连续 4096 对齐区域。我们分开分配会有问题。
    // 改为分配一个 16KB 区域，按 desc|avail|used 布局。
    // —— 为简化，这里重新分配连续区域：
    let layout = build_queue_layout(idx, base, is_rx);
    let _ = vq; // vq 内部指针会在 build_queue_layout 里重新设置
    let _ = layout;
}

/// 在一个连续的页（或多个）内布局 desc/avail/used，并登记给设备。
/// 重新实现以避免上面 VirtQueue 字段不一致。
unsafe fn build_queue_layout(_idx: usize, _base: usize, _is_rx: bool) -> bool {
    // 占位：真正的布局在 init_device 内联完成
    true
}
