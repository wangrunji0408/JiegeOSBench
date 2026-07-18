//! virtio-mmio 传输层与 virtio-net 网卡驱动（modern, version 2）

use crate::mm::{frame_alloc, FrameTracker, PhysPageNum};
use crate::sync::UPIntrFreeCell;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use lazy_static::lazy_static;

const MMIO_BASE: usize = 0x1000_1000;
const MMIO_SLOT_SIZE: usize = 0x1000;
const MMIO_SLOTS: usize = 8;

// 寄存器偏移
const REG_MAGIC: usize = 0x00;
const REG_VERSION: usize = 0x04;
const REG_DEVICE_ID: usize = 0x08;
const REG_DEV_FEATURES: usize = 0x10;
const REG_DEV_FEATURES_SEL: usize = 0x14;
const REG_DRV_FEATURES: usize = 0x20;
const REG_DRV_FEATURES_SEL: usize = 0x24;
const REG_QUEUE_SEL: usize = 0x30;
const REG_QUEUE_NUM_MAX: usize = 0x34;
const REG_QUEUE_NUM: usize = 0x38;
const REG_QUEUE_READY: usize = 0x44;
const REG_QUEUE_NOTIFY: usize = 0x50;
const REG_INTERRUPT_STATUS: usize = 0x60;
const REG_INTERRUPT_ACK: usize = 0x64;
const REG_STATUS: usize = 0x70;
const REG_QUEUE_DESC_LOW: usize = 0x80;
const REG_QUEUE_DESC_HIGH: usize = 0x84;
const REG_QUEUE_AVAIL_LOW: usize = 0x90;
const REG_QUEUE_AVAIL_HIGH: usize = 0x94;
const REG_QUEUE_USED_LOW: usize = 0xa0;
const REG_QUEUE_USED_HIGH: usize = 0xa4;
const REG_CONFIG: usize = 0x100;

const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const VIRTIO_F_VERSION_1: u64 = 1 << 32;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct Avail {
    flags: u16,
    idx: u16,
    ring: [u16; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct Used {
    flags: u16,
    idx: u16,
    ring: [UsedElem; 0],
}

pub struct VirtQueue {
    num: u16,
    desc: *mut Desc,
    avail: *mut Avail,
    used: *mut Used,
    free_head: u16,
    num_free: u16,
    last_used_idx: u16,
    _frame: FrameTracker, // 队列内存（需要 3 页：desc/avail/used 各一页以简化）
    _frame2: FrameTracker,
    _frame3: FrameTracker,
}

impl VirtQueue {
    fn new(num: u16) -> Self {
        let f1 = frame_alloc().expect("vq desc");
        let f2 = frame_alloc().expect("vq avail");
        let f3 = frame_alloc().expect("vq used");
        let desc = f1.ppn.as_ptr::<Desc>();
        let avail = f2.ppn.as_ptr::<Avail>();
        let used = f3.ppn.as_ptr::<Used>();
        // 初始化空闲描述符链
        for i in 0..num {
            unsafe {
                let d = &mut *desc.add(i as usize);
                d.next = i + 1;
                d.flags = 0;
            }
        }
        unsafe {
            (*desc.add((num - 1) as usize)).next = 0;
            (*avail).idx = 0;
            (*used).idx = 0;
        }
        Self {
            num,
            desc,
            avail,
            used,
            free_head: 0,
            num_free: num,
            last_used_idx: 0,
            _frame: f1,
            _frame2: f2,
            _frame3: f3,
        }
    }

    fn desc_pa(&self) -> usize {
        self.desc as usize
    }
    fn avail_pa(&self) -> usize {
        self.avail as usize
    }
    fn used_pa(&self) -> usize {
        self.used as usize
    }

    /// 分配一个描述符
    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let i = self.free_head;
        self.free_head = unsafe { (*self.desc.add(i as usize)).next };
        self.num_free -= 1;
        Some(i)
    }

    fn free_desc(&mut self, i: u16) {
        unsafe {
            (*self.desc.add(i as usize)).next = self.free_head;
        }
        self.free_head = i;
        self.num_free += 1;
    }

    /// 提交一个单描述符缓冲区，失败返回 None
    fn push_buf(&mut self, pa: usize, len: u32, device_write: bool) -> Option<u16> {
        let i = self.alloc_desc()?;
        unsafe {
            let d = &mut *self.desc.add(i as usize);
            d.addr = pa as u64;
            d.len = len;
            d.flags = if device_write { DESC_F_WRITE } else { 0 };
            d.next = 0;
            let avail = &mut *self.avail;
            let slot = avail.idx % self.num;
            let ring = avail.ring.as_mut_ptr();
            *ring.add(slot as usize) = i;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        Some(i)
    }

    /// 取回一个已用缓冲区，返回 (desc_id, len)
    fn pop_used(&mut self) -> Option<(u16, u32)> {
        unsafe {
            let used = &mut *self.used;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            if used.idx == self.last_used_idx {
                return None;
            }
            let slot = self.last_used_idx % self.num;
            let elem = *used.ring.as_ptr().add(slot as usize);
            self.last_used_idx = self.last_used_idx.wrapping_add(1);
            Some((elem.id as u16, elem.len))
        }
    }
}

/// virtio-net 设备
pub struct VirtioNet {
    base: usize,
    rx_queue: VirtQueue,
    tx_queue: VirtQueue,
    rx_bufs: Vec<Option<RxBuf>>,
    tx_bufs: Vec<Option<TxBuf>>,
    pub mac: [u8; 6],
}

struct RxBuf {
    _frame: FrameTracker,
    pa: usize,
}

struct TxBuf {
    _frame: FrameTracker,
    pa: usize,
}

const RX_BUF_SIZE: usize = 2048;
const TX_BUF_SIZE: usize = 2048;
const QUEUE_SIZE: u16 = 64;
/// virtio-net header（modern 12 字节）
pub const NET_HDR_SIZE: usize = 12;

impl VirtioNet {
    fn new(base: usize) -> Self {
        let read32 = |off: usize| unsafe { read_volatile((base + off) as *const u32) };
        let write32 = |off: usize, v: u32| unsafe { write_volatile((base + off) as *mut u32, v) };

        // 复位
        write32(REG_STATUS, 0);
        write32(REG_STATUS, STATUS_ACK);
        write32(REG_STATUS, STATUS_ACK | STATUS_DRIVER);
        // 特性协商：只要 VERSION_1
        write32(REG_DRV_FEATURES_SEL, 0);
        write32(REG_DRV_FEATURES, 0);
        write32(REG_DRV_FEATURES_SEL, 1);
        write32(REG_DRV_FEATURES, (VIRTIO_F_VERSION_1 >> 32) as u32);
        write32(REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        let status = read32(REG_STATUS);
        assert!(status & STATUS_FEATURES_OK != 0, "virtio-net features rejected");

        // 读 MAC
        let mut mac = [0u8; 6];
        for (i, b) in mac.iter_mut().enumerate() {
            *b = unsafe { read_volatile((base + REG_CONFIG + i) as *const u8) };
        }

        let rx_queue = VirtQueue::new(QUEUE_SIZE);
        let tx_queue = VirtQueue::new(QUEUE_SIZE);

        let mut dev = Self {
            base,
            rx_queue,
            tx_queue,
            rx_bufs: Vec::new(),
            tx_bufs: Vec::new(),
            mac,
        };

        // 配置队列
        dev.setup_queue(0, &dev.rx_queue);
        dev.setup_queue(1, &dev.tx_queue);

        write32(REG_STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);

        // 预投递 RX 缓冲区（desc id 即 rx_bufs 下标）
        for _ in 0..QUEUE_SIZE / 2 {
            let frame = frame_alloc().expect("rx buf");
            let pa = frame.ppn.as_ptr::<u8>() as usize;
            let id = dev
                .rx_queue
                .push_buf(pa, RX_BUF_SIZE as u32, true)
                .expect("rx desc");
            dev.rx_bufs.push(Some(RxBuf { _frame: frame, pa }));
            assert_eq!(id as usize, dev.rx_bufs.len() - 1);
        }
        // 剩余槽位置空（重投递时 id 可能落到这些槽位）
        for _ in QUEUE_SIZE / 2..QUEUE_SIZE {
            dev.rx_bufs.push(None);
        }
        // TX 缓冲池：每个 desc 一个固定缓冲区
        for _ in 0..QUEUE_SIZE {
            let frame = frame_alloc().expect("tx buf");
            let pa = frame.ppn.as_ptr::<u8>() as usize;
            dev.tx_bufs.push(Some(TxBuf { _frame: frame, pa }));
        }

        println!(
            "virtio-net: mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dev.mac[0], dev.mac[1], dev.mac[2], dev.mac[3], dev.mac[4], dev.mac[5]
        );
        dev
    }

    fn setup_queue(&self, idx: u16, q: &VirtQueue) {
        let write32 = |off: usize, v: u32| unsafe { write_volatile((self.base + off) as *mut u32, v) };
        write32(REG_QUEUE_SEL, idx as u32);
        write32(REG_QUEUE_NUM, q.num as u32);
        write32(REG_QUEUE_DESC_LOW, q.desc_pa() as u32);
        write32(REG_QUEUE_DESC_HIGH, (q.desc_pa() >> 32) as u32);
        write32(REG_QUEUE_AVAIL_LOW, q.avail_pa() as u32);
        write32(REG_QUEUE_AVAIL_HIGH, (q.avail_pa() >> 32) as u32);
        write32(REG_QUEUE_USED_LOW, q.used_pa() as u32);
        write32(REG_QUEUE_USED_HIGH, (q.used_pa() >> 32) as u32);
        write32(REG_QUEUE_READY, 1);
    }

    fn notify(&self, queue: u16) {
        unsafe { write_volatile((self.base + REG_QUEUE_NOTIFY) as *mut u32, queue as u32) };
    }

    /// 接收一个以太网帧（不含 virtio header），有数据时返回帧长度并拷贝到 buf
    pub fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        // 回收已用 RX 缓冲区
        while let Some((id, len)) = self.rx_queue.pop_used() {
            let rx = self.rx_bufs[id as usize].as_ref().expect("rx buf missing");
            let data_len = (len as usize).saturating_sub(NET_HDR_SIZE);
            let copy_len = core::cmp::min(data_len, buf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (rx.pa + NET_HDR_SIZE) as *const u8,
                    buf.as_mut_ptr(),
                    copy_len,
                );
            }
            // 复用同一描述符重新投递
            let pa = rx.pa;
            unsafe {
                let d = &mut *self.rx_queue.desc.add(id as usize);
                d.addr = pa as u64;
                d.len = RX_BUF_SIZE as u32;
                d.flags = DESC_F_WRITE;
                d.next = 0;
                let avail = &mut *self.rx_queue.avail;
                let slot = avail.idx % self.rx_queue.num;
                *avail.ring.as_mut_ptr().add(slot as usize) = id;
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
                avail.idx = avail.idx.wrapping_add(1);
            }
            self.notify(0);
            return Some(copy_len);
        }
        None
    }

    /// 发送一个以太网帧
    pub fn send(&mut self, data: &[u8]) -> Result<(), ()> {
        // 先回收已完成的 TX 描述符
        while let Some((id, _)) = self.tx_queue.pop_used() {
            self.tx_queue.free_desc(id);
        }
        if data.len() + NET_HDR_SIZE > TX_BUF_SIZE {
            return Err(());
        }
        if self.tx_queue.num_free == 0 {
            return Err(());
        }
        let id = self.tx_queue.alloc_desc().unwrap();
        let tx = self.tx_bufs[id as usize].as_ref().unwrap();
        unsafe {
            core::ptr::write_bytes(tx.pa as *mut u8, 0, NET_HDR_SIZE);
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                (tx.pa + NET_HDR_SIZE) as *mut u8,
                data.len(),
            );
            let d = &mut *self.tx_queue.desc.add(id as usize);
            d.addr = tx.pa as u64;
            d.len = (data.len() + NET_HDR_SIZE) as u32;
            d.flags = 0;
            d.next = 0;
            let avail = &mut *self.tx_queue.avail;
            let slot = avail.idx % self.tx_queue.num;
            *avail.ring.as_mut_ptr().add(slot as usize) = id;
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            avail.idx = avail.idx.wrapping_add(1);
        }
        self.notify(1);
        Ok(())
    }
}

lazy_static! {
    static ref NET_DEVICE: UPIntrFreeCell<Option<VirtioNet>> =
        unsafe { UPIntrFreeCell::new(None) };
}

pub fn init() {
    for i in 0..MMIO_SLOTS {
        let base = MMIO_BASE + i * MMIO_SLOT_SIZE;
        let magic = unsafe { read_volatile((base + REG_MAGIC) as *const u32) };
        if magic != 0x7472_6976 {
            continue;
        }
        let version = unsafe { read_volatile((base + REG_VERSION) as *const u32) };
        let device_id = unsafe { read_volatile((base + REG_DEVICE_ID) as *const u32) };
        if device_id == 0 {
            continue;
        }
        println!("virtio-mmio slot {}: device_id={} version={}", i, device_id, version);
        if device_id == 1 && version == 2 {
            let dev = VirtioNet::new(base);
            *NET_DEVICE.lock() = Some(dev);
            return;
        }
    }
    println!("warning: no virtio-net device found");
}

pub fn with_net_device<R>(f: impl FnOnce(&mut VirtioNet) -> R) -> Option<R> {
    let mut guard = NET_DEVICE.lock();
    guard.as_mut().map(f)
}
