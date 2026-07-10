/// VirtIO网络设备驱动
/// 提供发送/接收以太网帧的接口

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use crate::mm::alloc_frame;

// VirtIO Net特性位
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;

// VirtIO MMIO寄存器
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
const CONFIG: usize = 0x100;

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

// VirtIO Net头部（12字节）
#[repr(C)]
// VirtIO Net MMIO legacy header (version 1, 10 bytes)
// For version 2 (modern), an additional num_buffers field is added
struct NetHeader {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
}

// Legacy VirtIO net header size (no num_buffers)
const NET_HEADER_SIZE: usize = 10;

pub struct VirtioNet {
    base: usize,
    pub mac: [u8; 6],

    // 接收队列（queue 0）
    rx_desc: *mut [VirtqDesc; VIRTQ_SIZE],
    rx_avail: *mut VirtqAvail,
    rx_used: *mut VirtqUsed,
    rx_last_used: u16,
    rx_bufs: [usize; VIRTQ_SIZE], // 每个描述符的物理地址

    // 发送队列（queue 1）
    tx_desc: *mut [VirtqDesc; VIRTQ_SIZE],
    tx_avail: *mut VirtqAvail,
    tx_used: *mut VirtqUsed,
    tx_last_used: u16,
    tx_free: u16, // 空闲描述符索引

    // 接收的数据包缓冲区
    pub rx_queue: VecDeque<Vec<u8>>,
}

unsafe impl Send for VirtioNet {}
unsafe impl Sync for VirtioNet {}

pub static NET_DEVICE: Mutex<Option<VirtioNet>> = Mutex::new(None);

fn read_reg(base: usize, off: usize) -> u32 {
    unsafe { ((base + off) as *const u32).read_volatile() }
}

fn write_reg(base: usize, off: usize, val: u32) {
    unsafe { ((base + off) as *mut u32).write_volatile(val) }
}

pub fn init_net_device(base: usize, version: u32) {
    // 重置
    write_reg(base, STATUS, 0);
    write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

    // 读取MAC地址
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = unsafe { ((base + CONFIG + i) as *const u8).read_volatile() };
    }
    println!("[net] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    println!("[net] VirtIO version: {}", version);

    // 协商特性
    let features = read_reg(base, DEVICE_FEATURES) as u64;
    let wanted = VIRTIO_NET_F_MAC;
    let accepted = features & wanted;
    write_reg(base, DRIVER_FEATURES, accepted as u32);

    if version == 1 {
        // Legacy: don't set FEATURES_OK (not supported in v1)
        write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    } else {
        write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        let status = read_reg(base, STATUS);
        if status & STATUS_FEATURES_OK == 0 {
            println!("[net] Features negotiation failed");
            return;
        }
    }

    // 初始化接收队列（queue 0）
    let (rx_desc_pa, rx_avail_pa, rx_used_pa) = setup_queue(base, 0, version);

    // 初始化发送队列（queue 1）
    let (tx_desc_pa, tx_avail_pa, tx_used_pa) = setup_queue(base, 1, version);

    if version == 1 {
        write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);
    } else {
        write_reg(base, STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);
    }

    let rx_desc_va = crate::utils::phys_to_virt(rx_desc_pa);
    let rx_avail_va = crate::utils::phys_to_virt(rx_avail_pa);
    let rx_used_va = crate::utils::phys_to_virt(rx_used_pa);
    let tx_desc_va = crate::utils::phys_to_virt(tx_desc_pa);
    let tx_avail_va = crate::utils::phys_to_virt(tx_avail_pa);
    let tx_used_va = crate::utils::phys_to_virt(tx_used_pa);

    let mut dev = VirtioNet {
        base,
        mac,
        rx_desc: rx_desc_va as *mut [VirtqDesc; VIRTQ_SIZE],
        rx_avail: rx_avail_va as *mut VirtqAvail,
        rx_used: rx_used_va as *mut VirtqUsed,
        rx_last_used: 0,
        rx_bufs: [0; VIRTQ_SIZE],
        tx_desc: tx_desc_va as *mut [VirtqDesc; VIRTQ_SIZE],
        tx_avail: tx_avail_va as *mut VirtqAvail,
        tx_used: tx_used_va as *mut VirtqUsed,
        tx_last_used: 0,
        tx_free: 0,
        rx_queue: VecDeque::new(),
    };

    // 预分配接收缓冲区
    dev.refill_rx();

    *NET_DEVICE.lock() = Some(dev);
    println!("[net] VirtIO net device initialized");
}

// Legacy VirtIO MMIO registers (version 1)
const GUEST_PAGE_SIZE: usize = 0x028;
const QUEUE_PFN: usize = 0x040;

fn setup_queue(base: usize, queue: u32, version: u32) -> (usize, usize, usize) {
    let num_max = read_reg(base, QUEUE_NUM_MAX) as usize;
    let num = VIRTQ_SIZE.min(num_max);
    write_reg(base, QUEUE_NUM, num as u32);

    if version == 1 {
        // Legacy VirtIO MMIO (version 1)
        // Layout: desc table at PFN*4096, avail at PFN*4096+16*num, used at PFN*4096+4096
        write_reg(base, GUEST_PAGE_SIZE, 4096);

        let desc_frame = alloc_frame().expect("no mem desc");
        let avail_frame = alloc_frame().expect("no mem avail");
        let used_frame = alloc_frame().expect("no mem used");

        let desc_pa = desc_frame.0.addr();
        let avail_pa = avail_frame.0.addr();
        let used_pa = used_frame.0.addr();

        // Write PFN of desc table (device uses this to find all rings)
        write_reg(base, QUEUE_PFN, (desc_pa / 4096) as u32);

        core::mem::forget(desc_frame);
        core::mem::forget(avail_frame);
        core::mem::forget(used_frame);

        // avail ring is within desc_frame (at offset 16*num)
        // used ring is at desc_pa + 4096 (= avail_frame.addr() since sequential allocation)
        let avail_pa_actual = desc_pa + 16 * num;
        let used_pa_actual = desc_pa + 4096;
        (desc_pa, avail_pa_actual, used_pa_actual)
    } else {
        // Modern VirtIO MMIO (version 2)
        let desc_frame = alloc_frame().expect("no mem");
        let avail_frame = alloc_frame().expect("no mem");
        let used_frame = alloc_frame().expect("no mem");

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

        core::mem::forget(desc_frame);
        core::mem::forget(avail_frame);
        core::mem::forget(used_frame);

        (desc_pa, avail_pa, used_pa)
    }
}

impl VirtioNet {
    fn refill_rx(&mut self) {
        let rx_desc = unsafe { &mut *self.rx_desc };
        let rx_avail = unsafe { &mut *self.rx_avail };

        for i in 0..VIRTQ_SIZE {
            if self.rx_bufs[i] == 0 {
                let frame = alloc_frame().expect("no mem for rx buf");
                let pa = frame.0.addr();
                core::mem::forget(frame);
                self.rx_bufs[i] = pa;

                rx_desc[i] = VirtqDesc {
                    addr: pa as u64,
                    len: 4096,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                };

                let avail_idx = rx_avail.idx as usize % VIRTQ_SIZE;
                rx_avail.ring[avail_idx] = i as u16;
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
                rx_avail.idx = rx_avail.idx.wrapping_add(1);
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            }
        }

        // 通知设备
        write_reg(self.base, QUEUE_NOTIFY, 0);
    }

    pub fn poll_rx(&mut self) {
        // Acknowledge any pending VirtIO interrupts
        let int_status = unsafe { ((self.base + INTERRUPT_STATUS) as *const u32).read_volatile() };
        if int_status != 0 {
            unsafe { ((self.base + INTERRUPT_ACK) as *mut u32).write_volatile(int_status); }
        }

        let rx_used = unsafe { &*self.rx_used };

        loop {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            if rx_used.idx == self.rx_last_used {
                break;
            }

            let used_idx = self.rx_last_used as usize % VIRTQ_SIZE;
            let elem = &rx_used.ring[used_idx];
            let desc_id = elem.id as usize;
            let len = elem.len as usize;

            self.rx_last_used = self.rx_last_used.wrapping_add(1);

            if len > NET_HEADER_SIZE {
                let buf_pa = self.rx_bufs[desc_id];
                let buf_va = crate::utils::phys_to_virt(buf_pa);
                let data = unsafe {
                    core::slice::from_raw_parts(
                        (buf_va + NET_HEADER_SIZE) as *const u8,
                        len - NET_HEADER_SIZE,
                    )
                };
                self.rx_queue.push_back(data.to_vec());
            }

            // 标记缓冲区为空（refill）
            self.rx_bufs[desc_id] = 0;
        }

        self.refill_rx();
    }

    pub fn send(&mut self, data: &[u8]) -> bool {
        let tx_desc = unsafe { &mut *self.tx_desc };
        let tx_avail = unsafe { &mut *self.tx_avail };

        // 需要2个描述符：头 + 数据
        // 简化：用一个连续缓冲区
        let frame = alloc_frame().expect("no mem for tx");
        let pa = frame.0.addr();
        let va = crate::utils::phys_to_virt(pa);

        // 写入VirtIO网络头（全零）
        unsafe {
            core::ptr::write_bytes(va as *mut u8, 0, NET_HEADER_SIZE);
        }

        let total_len = NET_HEADER_SIZE + data.len();
        unsafe {
            core::slice::from_raw_parts_mut((va + NET_HEADER_SIZE) as *mut u8, data.len())
                .copy_from_slice(data);
        }

        let desc_id = (self.tx_free as usize) % VIRTQ_SIZE;
        self.tx_free = self.tx_free.wrapping_add(1);

        tx_desc[desc_id] = VirtqDesc {
            addr: pa as u64,
            len: total_len as u32,
            flags: 0,
            next: 0,
        };

        let avail_idx = tx_avail.idx as usize % VIRTQ_SIZE;
        tx_avail.ring[avail_idx] = desc_id as u16;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        tx_avail.idx = tx_avail.idx.wrapping_add(1);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        write_reg(self.base, QUEUE_NOTIFY, 1);

        // 等待发送完成
        let tx_used = unsafe { &*self.tx_used };
        let old_used = self.tx_last_used;
        let timeout = 100000usize;
        for _ in 0..timeout {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            if tx_used.idx != old_used {
                self.tx_last_used = tx_used.idx;
                break;
            }
        }

        true
    }
}

pub fn handle_irq(base: usize) {
    if let Some(dev) = NET_DEVICE.lock().as_mut() {
        dev.poll_rx();
    }
}
