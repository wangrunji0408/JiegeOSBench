//! virtio-mmio 网卡驱动（virtio 1.0 modern，轮询模式，不使用中断）

use alloc::vec::Vec;

// MMIO 寄存器偏移
const MMIO_MAGIC: usize = 0x000;
const MMIO_VERSION: usize = 0x004;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_DEV_FEATURES: usize = 0x010; // 64 位
const MMIO_DRV_FEATURES: usize = 0x020; // 64 位
const MMIO_QUEUE_SEL: usize = 0x030;
const MMIO_QUEUE_NUM_MAX: usize = 0x034;
const MMIO_QUEUE_NUM: usize = 0x038;
const MMIO_QUEUE_READY: usize = 0x044;
const MMIO_QUEUE_NOTIFY: usize = 0x050;
const MMIO_INT_STATUS: usize = 0x060;
const MMIO_INT_ACK: usize = 0x064;
const MMIO_STATUS: usize = 0x070;
const MMIO_QUEUE_DESC_LOW: usize = 0x080;
const MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const MMIO_QUEUE_AVAIL_LOW: usize = 0x090;
const MMIO_QUEUE_AVAIL_HIGH: usize = 0x094;
const MMIO_QUEUE_USED_LOW: usize = 0x0a0;
const MMIO_QUEUE_USED_HIGH: usize = 0x0a4;
const MMIO_CONFIG: usize = 0x100;

// 设备状态位
const VIRTIO_ACK: u32 = 1;
const VIRTIO_DRIVER: u32 = 2;
const VIRTIO_FEATURES_OK: u32 = 8;
const VIRTIO_DRIVER_OK: u32 = 4;
const VIRTIO_FAILED: u32 = 128;

// 特性位
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
const VIRTIO_NET_F_MAC: u64 = 1 << 5;

// virtqueue 常量
const VRING_DESC_SIZE: usize = 16;
const RX_QUEUE: usize = 0;
const TX_QUEUE: usize = 1;
const QUEUE_SIZE: usize = 128;

// virtio_net_hdr 大小（v1，无 mergeable）
const NET_HDR_SIZE: usize = 12;
// 以太网帧最大长度
const FRAME_SIZE: usize = 1526;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

const DESC_FLAG_NEXT: u16 = 1;
const DESC_FLAG_WRITE: u16 = 2;

#[repr(C, align(2))]
pub struct Avail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_SIZE],
    pub used_event: u16,
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
pub struct Used {
    pub flags: u16,
    pub idx: u16,
    pub ring: [UsedElem; QUEUE_SIZE],
    pub avail_event: u16,
}

pub struct VirtioNet {
    base: usize,
    mac: [u8; 6],
    // 队列内存（对齐分配：用全局静态，避免堆对齐问题）
    rx_desc: &'static mut [Desc],
    rx_avail: &'static mut Avail,
    rx_used: &'static mut Used,
    tx_desc: &'static mut [Desc],
    tx_avail: &'static mut Avail,
    tx_used: &'static mut Used,
    // RX 缓冲
    rx_buffers: Vec<&'static mut [u8]>, // 每项长度 NET_HDR+FRAME
    rx_free: Vec<usize>,                // 空闲 desc 索引
    rx_used_seen: u16,
    // TX
    tx_buffers: Vec<&'static mut [u8]>,
    tx_free: Vec<usize>,
    tx_used_seen: u16,
}

#[inline]
fn mmio_read(base: usize, off: usize) -> u32 {
    unsafe { ((base + off) as *const u32).read_volatile() }
}
#[inline]
fn mmio_write(base: usize, off: usize, v: u32) {
    unsafe { ((base + off) as *mut u32).write_volatile(v) }
}

/// 分配对齐的大块静态内存（物理页拼接）
fn alloc_aligned_block(bytes: usize, align: usize) -> *mut u8 {
    let pages = (bytes + align + 0xfff) / 0x1000 + 1;
    let mut first = 0usize;
    let mut chain: Vec<usize> = Vec::new();
    for _ in 0..pages {
        let p = crate::pmm::alloc_page().expect("oom for virtio");
        chain.push(p);
    }
    // 找一个 align 对齐的起点（页内偏移为 0 的页天然 4096 对齐；>= 4096 的对齐取页号对齐）
    let need_pages = (bytes + 0xfff) / 0x1000;
    let mut start_idx = 0usize;
    if align > 0x1000 {
        // 找到第一个 align 对齐的页
        for (i, &p) in chain.iter().enumerate() {
            if p % align == 0 && i + need_pages <= chain.len() {
                start_idx = i;
                break;
            }
        }
    }
    first = chain[start_idx];
    let _ = first;
    // 校验连续性
    for i in 0..need_pages {
        assert_eq!(chain[start_idx + i], chain[start_idx] + i * 0x1000, "phys pages not contiguous");
    }
    chain[start_idx] as *mut u8
}

/// 探测并初始化 virtio-net。找不到返回 None。
pub fn probe() -> Option<VirtioNet> {
    // QEMU virt: virtio-mmio 从 0x10001000 开始，间隔 0x200，共 32 个槽位
    let mut base = 0usize;
    for i in 0..32 {
        let b = 0x1000_1000 + i * 0x200;
        if mmio_read(b, MMIO_MAGIC) == 0x7472_6976 && mmio_read(b, MMIO_VERSION) == 2 {
            if mmio_read(b, MMIO_DEVICE_ID) == 1 {
                base = b;
                break;
            }
        }
    }
    if base == 0 {
        return None;
    }

    // 复位
    mmio_write(base, MMIO_STATUS, 0);
    let mut status = VIRTIO_ACK;
    mmio_write(base, MMIO_STATUS, status);
    status |= VIRTIO_DRIVER;
    mmio_write(base, MMIO_STATUS, status);

    // 特性协商：只接受 VERSION_1 + MAC
    let mut features = (mmio_read(base, MMIO_DEV_FEATURES) as u64)
        | ((mmio_read(base, MMIO_DEV_FEATURES + 4) as u64) << 32);
    features &= VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC;
    mmio_write(base, MMIO_DRV_FEATURES, features as u32);
    mmio_write(base, MMIO_DRV_FEATURES + 4, (features >> 32) as u32);
    status |= VIRTIO_FEATURES_OK;
    mmio_write(base, MMIO_STATUS, status);
    if mmio_read(base, MMIO_STATUS) & VIRTIO_FEATURES_OK == 0 {
        return None;
    }

    // MAC
    let mut mac = [0u8; 6];
    for i in 0..6 {
        unsafe { mac[i] = ((base + MMIO_CONFIG + i) as *const u8).read_volatile() }
    }

    // 队列内存分配
    let rx_desc = alloc_aligned_block(VRING_DESC_SIZE * QUEUE_SIZE, 16) as *mut Desc;
    let rx_avail = alloc_aligned_block(core::mem::size_of::<Avail>(), 2) as *mut Avail;
    let rx_used = alloc_aligned_block(core::mem::size_of::<Used>(), 4) as *mut Used;
    let tx_desc = alloc_aligned_block(VRING_DESC_SIZE * QUEUE_SIZE, 16) as *mut Desc;
    let tx_avail = alloc_aligned_block(core::mem::size_of::<Avail>(), 2) as *mut Avail;
    let tx_used = alloc_aligned_block(core::mem::size_of::<Used>(), 4) as *mut Used;
    // 清零
    unsafe {
        core::ptr::write_bytes(rx_desc as *mut u8, 0, VRING_DESC_SIZE * QUEUE_SIZE);
        core::ptr::write_bytes(rx_avail as *mut u8, 0, core::mem::size_of::<Avail>());
        core::ptr::write_bytes(rx_used as *mut u8, 0, core::mem::size_of::<Used>());
        core::ptr::write_bytes(tx_desc as *mut u8, 0, VRING_DESC_SIZE * QUEUE_SIZE);
        core::ptr::write_bytes(tx_avail as *mut u8, 0, core::mem::size_of::<Avail>());
        core::ptr::write_bytes(tx_used as *mut u8, 0, core::mem::size_of::<Used>());
    }

    let setup_queue = |sel: usize, desc: usize, avail: usize, used: usize| {
        mmio_write(base, MMIO_QUEUE_SEL, sel as u32);
        let max = mmio_read(base, MMIO_QUEUE_NUM_MAX) as usize;
        assert!(max >= QUEUE_SIZE, "virtqueue too small");
        mmio_write(base, MMIO_QUEUE_NUM, QUEUE_SIZE as u32);
        mmio_write(base, MMIO_QUEUE_DESC_LOW, (desc & 0xffff_ffff) as u32);
        mmio_write(base, MMIO_QUEUE_DESC_HIGH, (desc >> 32) as u32);
        mmio_write(base, MMIO_QUEUE_AVAIL_LOW, (avail & 0xffff_ffff) as u32);
        mmio_write(base, MMIO_QUEUE_AVAIL_HIGH, (avail >> 32) as u32);
        mmio_write(base, MMIO_QUEUE_USED_LOW, (used & 0xffff_ffff) as u32);
        mmio_write(base, MMIO_QUEUE_USED_HIGH, (used >> 32) as u32);
        mmio_write(base, MMIO_QUEUE_READY, 1);
    };
    setup_queue(RX_QUEUE, rx_desc as usize, rx_avail as usize, rx_used as usize);
    setup_queue(TX_QUEUE, tx_desc as usize, tx_avail as usize, tx_used as usize);

    status |= VIRTIO_DRIVER_OK;
    mmio_write(base, MMIO_STATUS, status);
    let _ = VIRTIO_FAILED;

    // RX 缓冲
    let mut rx_buffers: Vec<&'static mut [u8]> = Vec::new();
    for _ in 0..QUEUE_SIZE {
        let p = crate::pmm::alloc_page().expect("oom rx buf");
        let buf: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(p as *mut u8, 0x1000) };
        rx_buffers.push(buf);
    }
    // TX 缓冲（每 desc 独立）
    let mut tx_buffers: Vec<&'static mut [u8]> = Vec::new();
    for _ in 0..QUEUE_SIZE {
        let p = crate::pmm::alloc_page().expect("oom tx buf");
        let buf: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(p as *mut u8, 0x1000) };
        tx_buffers.push(buf);
    }

    let mut net = VirtioNet {
        base,
        mac,
        rx_desc: unsafe { core::slice::from_raw_parts_mut(rx_desc, QUEUE_SIZE) },
        rx_avail: unsafe { &mut *(rx_avail as *mut Avail) },
        rx_used: unsafe { &mut *(rx_used as *mut Used) },
        tx_desc: unsafe { core::slice::from_raw_parts_mut(tx_desc, QUEUE_SIZE) },
        tx_avail: unsafe { &mut *(tx_avail as *mut Avail) },
        tx_used: unsafe { &mut *(tx_used as *mut Used) },
        rx_buffers,
        rx_free: (0..QUEUE_SIZE).collect(),
        rx_used_seen: 0,
        tx_buffers,
        tx_free: (0..QUEUE_SIZE).collect(),
        tx_used_seen: 0,
    };
    // 提交全部 RX 缓冲
    net.refill_rx();
    Some(net)
}

impl VirtioNet {
    fn refill_rx(&mut self) {
        let mut submitted = false;
        while let Some(idx) = self.rx_free.pop() {
            let buf_ptr = self.rx_buffers[idx].as_mut_ptr() as u64;
            self.rx_desc[idx] = Desc {
                addr: buf_ptr,
                len: (NET_HDR_SIZE + FRAME_SIZE) as u32,
                flags: DESC_FLAG_WRITE,
                next: 0,
            };
            // push avail
            unsafe {
                let ring_idx = self.rx_avail.idx as usize % QUEUE_SIZE;
                self.rx_avail.ring[ring_idx] = idx as u16;
                // memory barrier
                core::arch::asm!("fence iorw, iorw");
                self.rx_avail.idx = self.rx_avail.idx.wrapping_add(1);
            }
            submitted = true;
        }
        if submitted {
            mmio_write(self.base, MMIO_QUEUE_NOTIFY, RX_QUEUE as u32);
        }
    }

    /// 尝试收取一帧。返回去掉 virtio_net_hdr 后的以太网帧。
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        // 先清理 TX 完成标记，回收 TX desc
        self.reap_tx();
        let seen = self.rx_used_seen;
        let idx = self.rx_used.idx;
        if seen == idx {
            // 顺带再补一次 RX
            self.refill_rx();
            return None;
        }
        unsafe {
            let slot = (seen as usize) % QUEUE_SIZE;
            let elem = self.rx_used.ring[slot];
            self.rx_used_seen = seen.wrapping_add(1);
            let desc_idx = elem.id as usize;
            let total_len = elem.len as usize; // hdr + frame
            let frame_len = total_len.saturating_sub(NET_HDR_SIZE);
            let buf = &self.rx_buffers[desc_idx];
            let frame = &buf[NET_HDR_SIZE..NET_HDR_SIZE + frame_len.min(FRAME_SIZE)];
            let out = frame.to_vec();
            self.rx_free.push(desc_idx);
            self.refill_rx();
            Some(out)
        }
    }

    fn reap_tx(&mut self) {
        let seen = self.tx_used_seen;
        let idx = self.tx_used.idx;
        let mut n = seen;
        while n != idx {
            unsafe {
                let slot = (n as usize) % QUEUE_SIZE;
                let elem = self.tx_used.ring[slot];
                self.tx_free.push(elem.id as usize);
            }
            n = n.wrapping_add(1);
        }
        self.tx_used_seen = n;
    }

    /// 发送一帧（阻塞至 desc 可用；轮询回收）
    pub fn send(&mut self, frame: &[u8]) {
        if frame.len() > FRAME_SIZE {
            return;
        }
        // 确保 TX desc 可用
        let mut spins = 0;
        while self.tx_free.is_empty() {
            self.reap_tx();
            if self.tx_free.is_empty() {
                spins += 1;
                if spins > 100 {
                    return; // 放弃（TCP 会重传）
                }
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
            }
        }
        let idx = self.tx_free.pop().unwrap();
        unsafe {
            let buf = &mut self.tx_buffers[idx][..];
            buf[..NET_HDR_SIZE].fill(0);
            buf[NET_HDR_SIZE..NET_HDR_SIZE + frame.len()].copy_from_slice(frame);
            let addr = buf.as_ptr() as u64;
            self.tx_desc[idx] = Desc {
                addr,
                len: (NET_HDR_SIZE + frame.len()) as u32,
                flags: 0,
                next: 0,
            };
            let ring_idx = self.tx_avail.idx as usize % QUEUE_SIZE;
            self.tx_avail.ring[ring_idx] = idx as u16;
            core::arch::asm!("fence iorw, iorw");
            self.tx_avail.idx = self.tx_avail.idx.wrapping_add(1);
            mmio_write(self.base, MMIO_QUEUE_NOTIFY, TX_QUEUE as u32);
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// TX 是否有可用 desc（smoltcp 用来决定是否给出 TxToken）
    pub fn tx_available(&mut self) -> bool {
        self.reap_tx();
        !self.tx_free.is_empty()
    }
}
