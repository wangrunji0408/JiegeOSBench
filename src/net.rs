//! virtio-mmio 网络驱动（legacy 接口，split virtqueue）。
//! QEMU virt 平台通过 -device virtio-net-device 接到 virtio-mmio 总线。

use core::ptr::{read_volatile, write_volatile};
use crate::mm::frame::FRAME_ALLOCATOR;

const MMIO_MAGIC: u32 = 0x74726976;
const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
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
// modern (v2) 寄存器
const REG_QUEUE_READY: usize = 0x044;
const REG_QUEUE_DESC_LOW: usize = 0x080;
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_DRIVER_LOW: usize = 0x088;
const REG_QUEUE_DRIVER_HIGH: usize = 0x08c;
const REG_QUEUE_DEVICE_LOW: usize = 0x090;
const REG_QUEUE_DEVICE_HIGH: usize = 0x094;

const S_ACK: u32 = 1;
const S_DRIVER: u32 = 2;
const S_DRIVER_OK: u32 = 4;
const S_FEATURES_OK: u32 = 8;

pub const VQ_SIZE: usize = 256;
pub const PKT_BUF: usize = 1514;
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
#[derive(Clone, Copy)]
pub struct AvailRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; VQ_SIZE],
    pub used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UsedElem {
    pub idx: u32,
    pub len: u32,
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
pub struct UsedRing {
    pub flags: u16,
    pub idx: u16,
    pub ring: [UsedElem; VQ_SIZE],
    pub avail_event: u16,
}

/// 一个连续队列区域：desc | avail | (页对齐) used
pub struct VirtQueue {
    pub base_pa: usize,      // 整个队列区域物理地址（2 页）
    pub desc: *mut VqDesc,
    pub avail: *mut AvailRing,
    pub used: *mut UsedRing,
    pub num: usize,
    pub last_used: u16,
    pub last_avail: u16,
    pub free_head: u16,
    pub free_count: u16,
    // RX 专用：每个 desc 关联的缓冲区
    pub rx_bufs: [usize; VQ_SIZE],
    pub tx_bufs: [usize; VQ_SIZE],
}

impl VirtQueue {
    pub fn new() -> Option<Self> {
        // VQ_SIZE=256: desc 占 4096B(1页), avail@4096, used@8192（3 页）
        let base_pa = FRAME_ALLOCATOR.alloc_contig(3)?;
        let desc = base_pa as *mut VqDesc;
        let avail = (base_pa + 16 * VQ_SIZE) as *mut AvailRing; // = base + 4096
        let used = (base_pa + 8192) as *mut UsedRing;
        unsafe {
            let p = base_pa as *mut u8;
            for i in 0..(3 * 4096) {
                core::ptr::write_volatile(p.add(i), 0);
            }
        }
        let mut q = Self {
            base_pa,
            desc,
            avail,
            used,
            num: VQ_SIZE,
            last_used: 0,
            last_avail: 0,
            free_head: 0,
            free_count: VQ_SIZE as u16,
            rx_bufs: [0; VQ_SIZE],
            tx_bufs: [0; VQ_SIZE],
        };
        unsafe {
            for i in 0..VQ_SIZE {
                core::ptr::write_volatile(&mut (*q.desc.add(i)).next as *mut u16, (i as u16 + 1) % VQ_SIZE as u16);
            }
        }
        Some(q)
    }

    /// 取一个空闲 desc 索引
    pub fn alloc_desc(&mut self) -> Option<u16> {
        if self.free_count == 0 {
            return None;
        }
        let h = self.free_head;
        self.free_head = unsafe { (*self.desc.add(h as usize)).next };
        self.free_count -= 1;
        Some(h)
    }

    pub fn free_desc(&mut self, idx: u16) {
        unsafe { (*self.desc.add(idx as usize)).next = self.free_head; }
        self.free_head = idx;
        self.free_count += 1;
    }
}

pub struct NetDriver {
    pub base: usize,
    pub rx: VirtQueue,
    pub tx: VirtQueue,
    pub mac: [u8; 6],
}

static mut NET: Option<NetDriver> = None;

pub fn driver() -> &'static mut NetDriver {
    unsafe { NET.as_mut().expect("net not initialized") }
}

unsafe fn reg_w(base: usize, off: usize, val: u32) {
    write_volatile((base + off) as *mut u32, val);
}
unsafe fn reg_r(base: usize, off: usize) -> u32 {
    read_volatile((base + off) as *const u32)
}

pub fn init() {
    unsafe {
        // 启用 PLIC source 8（virtio-net @ 10008000 的中断）给 S-mode
        let plic = 0x0c00_0000usize;
        write_volatile((plic + 8 * 4) as *mut u32, 1); // priority
        let enable = read_volatile((plic + 0x2000) as *const u32) | (1 << 8);
        write_volatile((plic + 0x2000) as *mut u32, enable); // S-mode enable
        write_volatile((plic + 0x20_0000) as *mut u32, 0); // S-mode threshold

        for i in 0..32usize {
            let base = 0x1000_1000 + i * 0x1000;
            let magic = reg_r(base, REG_MAGIC);
            if magic != MMIO_MAGIC {
                continue;
            }
            let ver = reg_r(base, REG_VERSION);
            let devid = reg_r(base, REG_DEVICE_ID);
            crate::println!("[net] slot {} magic={:#x} ver={} devid={}", i, magic, ver, devid);
            if devid != 1 {
                continue;
            }
            crate::println!("[net] found virtio-net @ {:#x} ver={}", base, ver);
            // 先试 modern（强制 VERSION_1），再 legacy
            if init_device_modern(base) {
                return;
            }
            crate::println!("[net] modern failed, trying legacy");
            if init_device(base) {
                return;
            }
        }
        crate::println!("[net] no virtio-net found (use -device virtio-net-device)");
    }
}

unsafe fn init_device(base: usize) -> bool {
    reg_w(base, REG_STATUS, 0);
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER);
    reg_w(base, 0x014, 0); // DeviceFeaturesSel = 0 (读 bit 0..31)
    let feat = reg_r(base, REG_DEVICE_FEATURES);
    let want: u32 = (1 << 0) | (1 << 1) | (1 << 5) | (1 << 16); // CSUM + GUEST_CSUM + MAC + STATUS
    let negotiated = feat & want;
    reg_w(base, 0x024, 0); // DriverFeaturesSel = 0
    reg_w(base, REG_DRIVER_FEATURES, negotiated);

    let mut rx = match VirtQueue::new() { Some(q) => q, None => return false };
    let mut tx = match VirtQueue::new() { Some(q) => q, None => return false };

    setup_queue(base, 0, &rx);
    setup_queue(base, 1, &tx);

    // MAC
    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = read_volatile((base + REG_CONFIG + i) as *const u8);
    }
    // 链路状态（协商了 STATUS 时，config offset 6, bit0=LINK_UP）
    let link_status = read_volatile((base + REG_CONFIG + 6) as *const u8);
    crate::println!("[net] link_status={:#x}", link_status);

    NET = Some(NetDriver { base, rx, tx, mac });

    // 先投递 RX 缓冲，再 DRIVER_OK
    fill_rx_with_base(&mut NET.as_mut().unwrap().rx, base);

    reg_w(base, REG_STATUS, S_ACK | S_DRIVER | S_DRIVER_OK);
    let st = reg_r(base, REG_STATUS);
    crate::println!("[net] status={:#x} feat={:#x} neg={:#x}", st, feat, negotiated);
    true
}

unsafe fn init_device_modern(base: usize) -> bool {
    reg_w(base, REG_STATUS, 0);
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER);
    // 读 device features word0
    reg_w(base, 0x014, 0); // DeviceFeaturesSel = 0
    let feat0 = reg_r(base, REG_DEVICE_FEATURES);
    reg_w(base, 0x014, 1); // word1
    let feat1 = reg_r(base, REG_DEVICE_FEATURES);
    let want0: u32 = (1 << 5) | (1 << 16); // MAC + STATUS
    let want1: u32 = 1; // VIRTIO_F_VERSION_1（强制协商）
    let neg0 = feat0 & want0;
    let neg1 = want1; // 强制，不等 feat1
    reg_w(base, 0x024, 0); // DriverFeaturesSel = 0
    reg_w(base, REG_DRIVER_FEATURES, neg0);
    reg_w(base, 0x024, 1);
    reg_w(base, REG_DRIVER_FEATURES, neg1);
    reg_w(base, REG_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK);
    let st = reg_r(base, REG_STATUS);
    if st & S_FEATURES_OK == 0 {
        crate::println!("[net] modern FEATURES_OK not accepted (feat0={:#x} feat1={:#x})", feat0, feat1);
        return false;
    }

    let rx = match VirtQueue::new() { Some(q) => q, None => return false };
    let tx = match VirtQueue::new() { Some(q) => q, None => return false };

    setup_queue_modern(base, 0, &rx);
    setup_queue_modern(base, 1, &tx);

    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = read_volatile((base + REG_CONFIG + i) as *const u8);
    }
    NET = Some(NetDriver { base, rx, tx, mac });
    fill_rx_with_base(&mut NET.as_mut().unwrap().rx, base);

    reg_w(base, REG_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK);
    let st = reg_r(base, REG_STATUS);
    crate::println!("[net] modern status={:#x} feat0={:#x} feat1={:#x} neg0={:#x} neg1={:#x}", st, feat0, feat1, neg0, neg1);
    // 校验队列确实启用
    reg_w(base, REG_QUEUE_SEL, 0);
    if reg_r(base, REG_QUEUE_READY) != 1 {
        crate::println!("[net] modern RX queue not ready, fallback");
        return false;
    }
    true
}

unsafe fn setup_queue_modern(base: usize, idx: usize, vq: &VirtQueue) {
    reg_w(base, REG_QUEUE_SEL, idx as u32);
    reg_w(base, REG_QUEUE_READY, 0); // 先禁用
    let nmax = reg_r(base, REG_QUEUE_NUM_MAX) as usize;
    let n = nmax.min(vq.num);
    crate::println!("[net] mqueue {} nmax={} n={}", idx, nmax, n);
    let desc_pa = vq.base_pa;
    let avail_pa = vq.base_pa + 4096;
    let used_pa = vq.base_pa + 8192;
    reg_w(base, REG_QUEUE_NUM, n as u32);
    reg_w(base, REG_QUEUE_ALIGN, 4096);
    reg_w(base, REG_QUEUE_DESC_LOW, (desc_pa & 0xffffffff) as u32);
    reg_w(base, REG_QUEUE_DESC_HIGH, (desc_pa >> 32) as u32);
    reg_w(base, REG_QUEUE_DRIVER_LOW, (avail_pa & 0xffffffff) as u32);
    reg_w(base, REG_QUEUE_DRIVER_HIGH, (avail_pa >> 32) as u32);
    reg_w(base, REG_QUEUE_DEVICE_LOW, (used_pa & 0xffffffff) as u32);
    reg_w(base, REG_QUEUE_DEVICE_HIGH, (used_pa >> 32) as u32);
    core::arch::asm!("fence ow, ow");
    reg_w(base, REG_QUEUE_READY, 1);
    let rdy = reg_r(base, REG_QUEUE_READY);
    crate::println!("[net] mqueue {} ready={}", idx, rdy);
}

unsafe fn setup_queue(base: usize, idx: usize, vq: &VirtQueue) {
    reg_w(base, REG_QUEUE_SEL, idx as u32);
    let nmax = reg_r(base, REG_QUEUE_NUM_MAX) as usize;
    let n = nmax.min(vq.num);
    crate::println!("[net] queue {} nmax={} n={} pfn={:#x}", idx, nmax, n, vq.base_pa >> 12);
    reg_w(base, REG_QUEUE_NUM, n as u32);
    reg_w(base, REG_QUEUE_ALIGN, 4096);
    reg_w(base, REG_QUEUE_PFN, (vq.base_pa >> 12) as u32);
    // 某些 QEMU 版本同时检查 QueueReady
    reg_w(base, REG_QUEUE_READY, 1);
    let pfn_back = reg_r(base, REG_QUEUE_PFN);
    crate::println!("[net] queue {} pfn_back={:#x}", idx, pfn_back);
}

/// 向 RX 队列投递空缓冲
pub fn fill_rx(vq: &mut VirtQueue) {
    let base = unsafe { driver().base };
    fill_rx_with_base(vq, base);
}

pub fn fill_rx_with_base(vq: &mut VirtQueue, base: usize) {
    let mut posted = 0;
    for i in 0..vq.num {
        if vq.rx_bufs[i] != 0 {
            continue;
        }
        let buf = match FRAME_ALLOCATOR.alloc_zeroed() {
            Some(b) => b,
            None => return,
        };
        vq.rx_bufs[i] = buf;
        unsafe {
            core::ptr::write_volatile(&mut (*vq.desc.add(i)).addr as *mut u64, buf as u64);
            core::ptr::write_volatile(&mut (*vq.desc.add(i)).len as *mut u32, (NET_HDR_LEN + PKT_BUF) as u32);
            core::ptr::write_volatile(&mut (*vq.desc.add(i)).flags as *mut u16, 2);
            core::ptr::write_volatile(&mut (*vq.desc.add(i)).next as *mut u16, 0);
            let a = vq.avail;
            let idx = core::ptr::read_volatile(&(*a).idx);
            core::ptr::write_volatile((*a).ring.as_mut_ptr().add(idx as usize % vq.num), i as u16);
            core::ptr::write_volatile(&mut (*a).idx as *mut u16, idx.wrapping_add(1));
        }
        posted += 1;
    }
    crate::println!("[net] posted {} rx buffers, avail idx={}", posted, unsafe{(*vq.avail).idx});
    unsafe {
        core::arch::asm!("fence ow, ow");
        reg_w(base, REG_QUEUE_NOTIFY, 0);
    }
}

/// 重新通知 RX 队列（设备可能错过初始 notify）
pub fn kick_rx() {
    unsafe {
        if let Some(d) = NET.as_mut() {
            core::arch::asm!("fence ow, ow");
            reg_w(d.base, REG_QUEUE_NOTIFY, 0);
        }
    }
}

/// 发送一个 gratuitous ARP，让 slirp 网关学习本机 MAC
pub fn send_gratuitous_arp() {
    let mac = match unsafe { NET.as_ref() } { Some(d) => d.mac, None => return };
    let mut pkt = [0u8; 42];
    // 以太网头
    pkt[0..6].copy_from_slice(&[0xff; 6]); // 广播
    pkt[6..12].copy_from_slice(&mac);
    pkt[12] = 0x08; pkt[13] = 0x06; // ARP
    // ARP
    pkt[14] = 0x00; pkt[15] = 0x01; // hardware = ethernet
    pkt[16] = 0x08; pkt[17] = 0x00; // proto = ipv4
    pkt[18] = 6; pkt[19] = 4; // HW len, proto len
    pkt[20] = 0x00; pkt[21] = 0x01; // opcode = request
    pkt[22..28].copy_from_slice(&mac);
    pkt[28..32].copy_from_slice(&IFACE_IP);
    pkt[32..38].copy_from_slice(&mac);
    pkt[38..42].copy_from_slice(&IFACE_IP);
    send_packet_raw(&pkt);
}

const IFACE_IP: [u8; 4] = [10, 0, 2, 15];

/// 发送原始以太网帧（含 net_hdr 前缀由 send_packet 处理）
fn send_packet_raw(data: &[u8]) {
    send_packet(data);
}

/// 发送一个以太网帧（含 net_hdr）。返回是否成功投递。
pub fn send_packet(data: &[u8]) -> bool {
    let d = unsafe { driver() };
    let i = match d.tx.alloc_desc() {
        Some(i) => i,
        None => return false,
    };
    // 分配缓冲：net_hdr + data
    let buf = match FRAME_ALLOCATOR.alloc_zeroed() {
        Some(b) => b,
        None => {
            d.tx.free_desc(i);
            return false;
        }
    };
    d.tx.tx_bufs[i as usize] = buf;
    crate::println!("[net] TX desc {} buf={:#x} len={}", i, buf, data.len());
    unsafe {
        for k in 0..NET_HDR_LEN {
            core::ptr::write_volatile((buf + k) as *mut u8, 0);
        }
        let copy = data.len().min(PKT_BUF);
        core::ptr::copy_nonoverlapping(data.as_ptr(), (buf + NET_HDR_LEN) as *mut u8, copy);
        core::ptr::write_volatile(&mut (*d.tx.desc.add(i as usize)).addr as *mut u64, buf as u64);
        core::ptr::write_volatile(&mut (*d.tx.desc.add(i as usize)).len as *mut u32, (NET_HDR_LEN + copy) as u32);
        core::ptr::write_volatile(&mut (*d.tx.desc.add(i as usize)).flags as *mut u16, 0);
        core::ptr::write_volatile(&mut (*d.tx.desc.add(i as usize)).next as *mut u16, 0);
        let a = d.tx.avail;
        let idx = core::ptr::read_volatile(&(*a).idx);
        core::ptr::write_volatile((*a).ring.as_mut_ptr().add(idx as usize % d.tx.num), i);
        core::ptr::write_volatile(&mut (*a).idx as *mut u16, idx.wrapping_add(1));
        core::arch::asm!("fence ow, ow");
    }
    unsafe { reg_w(d.base, REG_QUEUE_NOTIFY, 1); }
    // 检查 TX used 是否推进
    let tx_used = unsafe { (*d.tx.used).idx };
    crate::println!("[net] TX avail.idx={} used.idx={}", unsafe { (*d.tx.avail).idx }, tx_used);
    true
}

/// 轮询收取已完成的 RX 包。对每个包调用 cb(data)。
pub fn recv_packets<F: FnMut(&[u8])>(mut cb: F) {
    let d = unsafe { driver() };
    unsafe { core::arch::asm!("fence r, rw"); }
    let used_idx = unsafe { (*d.rx.used).idx };
    let ist = unsafe { reg_r(d.base, REG_INT_STATUS) };
    if used_idx != d.rx.last_used || ist != 0 {
        crate::println!("[net] used.idx={} last_used={} intst={:#x}", used_idx, d.rx.last_used, ist);
    }
    loop {
        let used_idx = unsafe { (*d.rx.used).idx };
        if d.rx.last_used == used_idx {
            break;
        }
        let ue = unsafe {
            &(*d.rx.used).ring[d.rx.last_used as usize % d.rx.num]
        };
        let desc_idx = ue.idx as usize;
        let len = ue.len as usize;
        d.rx.last_used = d.rx.last_used.wrapping_add(1);
        if len < NET_HDR_LEN {
            // 回收
            reclaim_rx(d, desc_idx);
            continue;
        }
        let buf_pa = d.rx.rx_bufs[desc_idx];
        let data_len = len - NET_HDR_LEN;
        let data = unsafe {
            core::slice::from_raw_parts((buf_pa + NET_HDR_LEN) as *const u8, data_len)
        };
        cb(data);
        // 回收并重新投递
        reclaim_rx(d, desc_idx);
        // 重新投递该缓冲
        unsafe {
            let a = &mut *d.rx.avail;
            a.ring[a.idx as usize % d.rx.num] = desc_idx as u16;
            a.idx = a.idx.wrapping_add(1);
        }
    }
    unsafe { reg_w(d.base, REG_QUEUE_NOTIFY, 0); }
}

fn reclaim_rx(_d: &mut NetDriver, _i: usize) {
    // desc 已被设备使用，这里不释放缓冲（缓冲复用）
}

/// 处理外部中断（virtio-net）
pub fn irq_handler() {
    let d = unsafe { driver() };
    unsafe {
        let st = reg_r(d.base, REG_INT_STATUS);
        if st != 0 {
            reg_w(d.base, REG_INT_ACK, st);
        }
    }
}

/// 检查 TX 完成情况，回收 desc
pub fn poll_tx() {
    let d = unsafe { driver() };
    unsafe { core::arch::asm!("fence r, rw"); }
    let tx_used = unsafe { core::ptr::read_volatile(&(*d.tx.used).idx as *const u16) };
    if tx_used != d.tx.last_used {
        crate::println!("[net] TX used.idx={} last={}", tx_used, d.tx.last_used);
    }
    while d.tx.last_used != tx_used {
        let ue = unsafe { &(*d.tx.used).ring[d.tx.last_used as usize % d.tx.num] };
        let desc_idx = ue.idx as u16;
        d.tx.last_used = d.tx.last_used.wrapping_add(1);
        d.tx.free_desc(desc_idx);
    }
}

/// 调试：dump TX 队列状态
pub fn dump_tx() {
    let d = unsafe { driver() };
    unsafe {
        let desc0 = &*d.tx.desc;
        let a = &*d.tx.avail;
        let u = &*d.tx.used;
        crate::println!(
            "[net] TX desc0 addr={:#x} len={} flags={} next={}",
            desc0.addr, desc0.len, desc0.flags, desc0.next
        );
        crate::println!(
            "[net] TX avail flags={} idx={} ring[0]={} used_event={}",
            a.flags, a.idx, a.ring[0], a.used_event
        );
        crate::println!(
            "[net] TX used flags={} idx={}",
            u.flags, u.idx
        );
        // dump used 环前 32 字节
        let p = d.tx.used as *const u8;
        let mut s = alloc::string::String::new();
        for i in 0..32 {
            s.push_str(&alloc::format!("{:02x} ", core::ptr::read_volatile(p.add(i))));
        }
        crate::println!("[net] TX used raw: {}", s);
    }
}
