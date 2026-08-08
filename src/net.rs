use core::ptr;

use crate::console;

static mut VIRTIO: usize = 0x1000_1000;
const Q: usize = 8;
const BUF: usize = 2048;
const PAGE: usize = 4096;
const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
const TCP_MSS: usize = 1400;

const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc { addr: u64, len: u32, flags: u16, next: u16 }

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElem { id: u32, len: u32 }

#[repr(C)]
struct Avail { flags: u16, idx: u16, ring: [u16; Q], used_event: u16 }

#[repr(C)]
struct Used { flags: u16, idx: u16, ring: [UsedElem; Q], avail_event: u16 }

#[repr(C, align(4096))]
struct Queue {
    desc: [Desc; Q],
    avail: Avail,
    _legacy_padding: [u8; PAGE - core::mem::size_of::<[Desc; Q]>() - core::mem::size_of::<Avail>()],
    used: Used,
    buffers: [[u8; BUF]; Q],
}

const EMPTY_DESC: Desc = Desc { addr: 0, len: 0, flags: 0, next: 0 };
const EMPTY_USED: UsedElem = UsedElem { id: 0, len: 0 };
static mut RX: Queue = Queue {
    desc: [EMPTY_DESC; Q],
    avail: Avail { flags: 0, idx: 0, ring: [0; Q], used_event: 0 },
    _legacy_padding: [0; PAGE - core::mem::size_of::<[Desc; Q]>() - core::mem::size_of::<Avail>()],
    used: Used { flags: 0, idx: 0, ring: [EMPTY_USED; Q], avail_event: 0 },
    buffers: [[0; BUF]; Q],
};
static mut TX: Queue = Queue {
    desc: [EMPTY_DESC; Q],
    avail: Avail { flags: 0, idx: 0, ring: [0; Q], used_event: 0 },
    _legacy_padding: [0; PAGE - core::mem::size_of::<[Desc; Q]>() - core::mem::size_of::<Avail>()],
    used: Used { flags: 0, idx: 0, ring: [EMPTY_USED; Q], avail_event: 0 },
    buffers: [[0; BUF]; Q],
};
static mut RX_LAST_USED: u16 = 0;
static mut TX_LAST_USED: u16 = 0;
static mut TX_SLOT: usize = 0;
static mut INITIALIZED: bool = false;
static mut LEGACY: bool = false;
static mut MAC: [u8; 6] = [0; 6];

#[derive(Clone, Copy, PartialEq)]
enum State { Free, SynReceived, Established, Closed }

#[derive(Clone, Copy)]
struct Conn {
    state: State,
    listen_socket: usize,
    local_port: u16,
    remote_port: u16,
    remote_ip: [u8; 4],
    peer_mac: [u8; 6],
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    rx_len: usize,
    rx: [u8; 4096],
    accepted: bool,
}

const EMPTY_CONN: Conn = Conn {
    state: State::Free, listen_socket: 0, local_port: 0, remote_port: 0,
    remote_ip: [0; 4], peer_mac: [0; 6], iss: 0, snd_una: 0, snd_nxt: 0,
    rcv_nxt: 0, rx_len: 0, rx: [0; 4096], accepted: false,
};
static mut CONNS: [Conn; 8] = [EMPTY_CONN; 8];
static mut LISTEN_PORT: [u16; 8] = [0; 8];
static mut LISTEN_USED: [bool; 8] = [false; 8];
static mut NEXT_ISN: u32 = 0x1020_3040;

#[inline]
unsafe fn mmio_read32(offset: usize) -> u32 { ptr::read_volatile((VIRTIO + offset) as *const u32) }
#[inline]
unsafe fn mmio_write32(offset: usize, value: u32) { ptr::write_volatile((VIRTIO + offset) as *mut u32, value); }
#[inline]
unsafe fn mmio_read16(offset: usize) -> u16 { ptr::read_volatile((VIRTIO + offset) as *const u16) }
#[inline]
unsafe fn mmio_write16(offset: usize, value: u16) { ptr::write_volatile((VIRTIO + offset) as *mut u16, value); }

fn set_addr(low: usize, high: usize, addr: usize) {
    unsafe { mmio_write32(low, addr as u32); mmio_write32(high, (addr >> 32) as u32); }
}

pub fn init() -> bool {
    unsafe {
        let mut found = false;
        for slot in 1..=8 {
            let base = 0x1000_0000 + slot * 0x1000;
            let magic = ptr::read_volatile(base as *const u32);
            let device = ptr::read_volatile((base + 0x008) as *const u32);
            if magic == 0x7472_6976 && device == 1 { VIRTIO = base; found = true; break; }
        }
        if !found {
            console::write_str("Luna: virtio-net not found\n");
            return false;
        }
        LEGACY = mmio_read32(0x004) == 1;
        console::write_str("virtio base="); console::write_hex(VIRTIO);
        console::write_str(" version="); console::write_hex(mmio_read32(0x004) as usize);
        console::write_str(" status="); console::write_hex(mmio_read32(0x070) as usize); console::write_str("\n");
        for i in 0..6 { MAC[i] = ptr::read_volatile((VIRTIO + 0x100 + i) as *const u8); }
        mmio_write32(0x070, 0);
        mmio_write32(0x070, 1);
        mmio_write32(0x070, 3);
        mmio_write32(0x014, 0);
        let f0 = mmio_read32(0x010);
        mmio_write32(0x014, 1);
        let f1 = mmio_read32(0x010);
        let offered = (f0 as u64) | ((f1 as u64) << 32);
        let wanted = offered & (1u64 << 32); // VERSION_1
        mmio_write32(0x024, 0); mmio_write32(0x020, wanted as u32);
        if !LEGACY {
            mmio_write32(0x024, 1); mmio_write32(0x020, (wanted >> 32) as u32);
        }
        mmio_write32(0x070, 11);
        if mmio_read32(0x070) & 8 == 0 { console::write_str("Luna: virtio feature negotiation failed\n"); return false; }
        if LEGACY { mmio_write32(0x028, PAGE as u32); }
        setup_queue(0, &raw mut RX, LEGACY);
        setup_queue(1, &raw mut TX, LEGACY);
        mmio_write32(0x070, 15);
        // Tell the device that the initially posted RX buffers are available.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        mmio_write32(0x050, 0);
        console::write_str("virtio status="); console::write_hex(mmio_read32(0x070) as usize); console::write_str("\n");
        INITIALIZED = true;
        console::write_str("Luna: virtio-net MAC ");
        for i in 0..6 { console::write_hex_byte(MAC[i]); if i != 5 { console::write_str(":"); } }
        console::write_str("\n");
        true
    }
}

unsafe fn setup_queue(index: u32, q: *mut Queue, legacy: bool) {
    mmio_write32(0x030, index);
    let max = mmio_read32(0x034) as usize;
    if max < Q { console::write_str("Luna: virtqueue too small\n"); return; }
    mmio_write32(0x038, Q as u32);
    if index == 0 {
        for i in 0..Q {
            (*q).desc[i] = Desc { addr: (&raw mut (*q).buffers[i]) as *mut _ as usize as u64, len: BUF as u32, flags: DESC_WRITE, next: 0 };
            (*q).avail.ring[i] = i as u16;
        }
        (*q).avail.idx = Q as u16;
    }
    if legacy {
        mmio_write32(0x03c, PAGE as u32);
        mmio_write32(0x040, ((&raw mut (*q).desc) as *mut _ as usize / PAGE) as u32);
    } else {
        set_addr(0x080, 0x084, (&raw mut (*q).desc) as *mut _ as usize);
        set_addr(0x090, 0x094, (&raw mut (*q).avail) as *mut _ as usize);
        set_addr(0x0a0, 0x0a4, (&raw mut (*q).used) as *mut _ as usize);
        mmio_write32(0x044, 1);
    }
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() { sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32; i += 2; }
    if i < data.len() { sum += (data[i] as u32) << 8; }
    while (sum >> 16) != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    !(sum as u16)
}

fn tcp_checksum(src: [u8; 4], dst: [u8; 4], tcp: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in src.chunks_exact(2) { sum += u16::from_be_bytes([pair[0], pair[1]]) as u32; }
    for pair in dst.chunks_exact(2) { sum += u16::from_be_bytes([pair[0], pair[1]]) as u32; }
    sum += 6 + tcp.len() as u32;
    let mut i = 0;
    while i + 1 < tcp.len() { sum += u16::from_be_bytes([tcp[i], tcp[i + 1]]) as u32; i += 2; }
    if i < tcp.len() { sum += (tcp[i] as u32) << 8; }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    !(sum as u16)
}

fn send_frame(frame: &[u8]) {
    unsafe {
        if !INITIALIZED || frame.len() + 10 > BUF { return; }
        reclaim_tx();
        let slot = TX_SLOT;
        TX_SLOT = (TX_SLOT + 1) % Q;
        console::write_str("net tx len="); console::write_dec(frame.len()); console::write_str(" slot="); console::write_dec(slot); console::write_str("\n");
        let b = &mut TX.buffers[slot];
        for x in &mut b[..10] { *x = 0; }
        b[10..10 + frame.len()].copy_from_slice(frame);
        TX.desc[slot] = Desc { addr: b.as_ptr() as usize as u64, len: (10 + frame.len()) as u32, flags: 0, next: 0 };
        let idx = TX.avail.idx as usize % Q;
        TX.avail.ring[idx] = slot as u16;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        TX.avail.idx = TX.avail.idx.wrapping_add(1);
        mmio_write32(0x030, 1); mmio_write32(0x050, 0);
    }
}

unsafe fn reclaim_tx() {
    let idx = TX.used.idx;
    while TX_LAST_USED != idx {
        TX_LAST_USED = TX_LAST_USED.wrapping_add(1);
    }
}

pub fn poll() {
    unsafe {
        if !INITIALIZED { return; }
        let used = RX.used.idx;
        while RX_LAST_USED != used {
            let e = RX.used.ring[RX_LAST_USED as usize % Q];
            RX_LAST_USED = RX_LAST_USED.wrapping_add(1);
            let id = e.id as usize;
            let len = (e.len as usize).min(BUF);
            console::write_str("net rx len="); console::write_dec(len); console::write_str("\n");
            if len > 10 { handle_packet(&RX.buffers[id][10..len]); }
            let a = RX.avail.idx as usize % Q;
            RX.avail.ring[a] = id as u16;
            RX.avail.idx = RX.avail.idx.wrapping_add(1);
            mmio_write32(0x030, 0); mmio_write32(0x050, 0);
        }
    }
}

fn arp_reply(frame: &[u8]) {
    if frame.len() < 42 { return; }
    console::write_str("arp op="); console::write_hex(u16::from_be_bytes([frame[20], frame[21]]) as usize);
    console::write_str("arp tpa=");
    for x in &frame[38..42] { console::write_dec(*x as usize); console::write_str("."); }
    console::write_str("\n");
    if frame[20] != 0 || frame[21] != 1 { return; }
    if frame[38..42] != GUEST_IP { return; }
    let mut out = [0u8; 42];
    out[0..6].copy_from_slice(&frame[6..12]);
    unsafe { out[6..12].copy_from_slice(&MAC); }
    out[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    out[14..16].copy_from_slice(&1u16.to_be_bytes());
    out[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
    out[18] = 6; out[19] = 4; out[20..22].copy_from_slice(&2u16.to_be_bytes());
    unsafe { out[22..28].copy_from_slice(&MAC); }
    out[28..32].copy_from_slice(&GUEST_IP);
    out[32..38].copy_from_slice(&frame[22..28]);
    out[38..42].copy_from_slice(&frame[28..32]);
    send_frame(&out);
}

fn send_tcp(id: usize, flags: u8, payload: &[u8]) {
    unsafe {
        let c = &mut CONNS[id];
        let mut tcp = [0u8; 1500];
        tcp[0..2].copy_from_slice(&c.local_port.to_be_bytes());
        tcp[2..4].copy_from_slice(&c.remote_port.to_be_bytes());
        tcp[4..8].copy_from_slice(&c.snd_nxt.to_be_bytes());
        tcp[8..12].copy_from_slice(&c.rcv_nxt.to_be_bytes());
        tcp[12] = 5 << 4; tcp[13] = flags;
        tcp[14..16].copy_from_slice(&65535u16.to_be_bytes());
        tcp[18..20].copy_from_slice(&0u16.to_be_bytes());
        tcp[20..20 + payload.len()].copy_from_slice(payload);
        let len = 20 + payload.len();
        let sum = tcp_checksum(GUEST_IP, c.remote_ip, &tcp[..len]);
        tcp[16..18].copy_from_slice(&sum.to_be_bytes());
        let mut ip = [0u8; 20];
        ip[0] = 0x45; ip[2..4].copy_from_slice(&((20 + len) as u16).to_be_bytes());
        ip[4..6].copy_from_slice(&NEXT_ISN.to_be_bytes()[0..2]);
        NEXT_ISN = NEXT_ISN.wrapping_add(1);
        ip[8] = 64; ip[9] = 6; ip[12..16].copy_from_slice(&GUEST_IP); ip[16..20].copy_from_slice(&c.remote_ip);
        let ip_sum = checksum(&ip);
        ip[10..12].copy_from_slice(&ip_sum.to_be_bytes());
        let mut frame = [0u8; 1536];
        frame[0..6].copy_from_slice(&c.peer_mac);
        frame[6..12].copy_from_slice(&MAC);
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14..34].copy_from_slice(&ip);
        frame[34..34 + len].copy_from_slice(&tcp[..len]);
        send_frame(&frame[..34 + len]);
        if flags & 0x02 != 0 { c.snd_nxt = c.snd_nxt.wrapping_add(1); }
        if flags & 0x01 != 0 { c.snd_nxt = c.snd_nxt.wrapping_add(1); }
        c.snd_nxt = c.snd_nxt.wrapping_add(payload.len() as u32);
    }
}

fn tcp_input(frame: &[u8], ip: &[u8]) {
    if frame.len() < 54 { return; }
    let ihl = ((ip[0] & 0x0f) as usize) * 4;
    if frame.len() < 14 + ihl + 20 { return; }
    let tcp = &frame[14 + ihl..];
    let dst_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    let src_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    let ack = u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]);
    let flags = tcp[13];
    let hdr = ((tcp[12] >> 4) as usize) * 4;
    let payload = if hdr <= tcp.len() { &tcp[hdr..] } else { &[] };
    console::write_str("tcp in sport="); console::write_dec(src_port as usize);
    console::write_str(" dport="); console::write_dec(dst_port as usize);
    console::write_str(" flags="); console::write_hex(flags as usize);
    console::write_str(" seq="); console::write_hex(seq as usize);
    console::write_str(" ack="); console::write_hex(ack as usize);
    console::write_str(" payload="); console::write_dec(payload.len()); console::write_str("\n");
    unsafe {
        let mut found = None;
        for i in 0..CONNS.len() {
            if CONNS[i].state != State::Free && CONNS[i].local_port == dst_port && CONNS[i].remote_port == src_port && CONNS[i].remote_ip == [ip[12], ip[13], ip[14], ip[15]] { found = Some(i); break; }
        }
        let id = if let Some(i) = found { i } else if flags & 0x02 != 0 {
            let mut slot = None;
            for i in 0..CONNS.len() { if CONNS[i].state == State::Free { slot = Some(i); break; } }
            let i = match slot { Some(i) => i, None => return };
            let mut listen = None;
            for s in 0..LISTEN_USED.len() { if LISTEN_USED[s] && LISTEN_PORT[s] == dst_port { listen = Some(s); break; } }
            let s = match listen { Some(s) => s, None => return };
            NEXT_ISN = NEXT_ISN.wrapping_add(7919);
            CONNS[i] = Conn { state: State::SynReceived, listen_socket: s, local_port: dst_port, remote_port: src_port, remote_ip: [ip[12],ip[13],ip[14],ip[15]], peer_mac: [frame[6],frame[7],frame[8],frame[9],frame[10],frame[11]], iss: NEXT_ISN, snd_una: NEXT_ISN, snd_nxt: NEXT_ISN.wrapping_add(1), rcv_nxt: seq.wrapping_add(1), rx_len: 0, rx: [0;4096], accepted: false };
            send_tcp(i, 0x12, &[]);
            return;
        } else { return };
        let c = &mut CONNS[id];
        if c.state == State::SynReceived && flags & 0x10 != 0 && ack >= c.snd_nxt {
            c.state = State::Established;
        }
        if c.state != State::Established { return; }
        if ack > c.snd_una { c.snd_una = ack; }
        if !payload.is_empty() && seq == c.rcv_nxt {
            let n = payload.len().min(c.rx.len() - c.rx_len);
            c.rx[c.rx_len..c.rx_len+n].copy_from_slice(&payload[..n]);
            c.rx_len += n;
            c.rcv_nxt = c.rcv_nxt.wrapping_add(n as u32);
            send_tcp(id, 0x10, &[]);
        }
        if flags & 0x01 != 0 { c.rcv_nxt = c.rcv_nxt.wrapping_add(1); send_tcp(id, 0x11, &[]); c.state = State::Closed; }
    }
}

fn handle_packet(frame: &[u8]) {
    if frame.len() < 14 { return; }
    console::write_str("eth type="); console::write_hex(u16::from_be_bytes([frame[12], frame[13]]) as usize); console::write_str("\n");
    match u16::from_be_bytes([frame[12], frame[13]]) {
        0x0806 => arp_reply(frame),
        0x0800 if frame.len() >= 34 => {
            let ip = &frame[14..];
            if ip[9] == 6 && ip[16..20] == GUEST_IP { tcp_input(frame, ip); }
        }
        _ => {}
    }
}

pub fn bind(socket: usize, port: u16) -> isize {
    unsafe { if socket >= LISTEN_USED.len() || LISTEN_USED[socket] { return -98; } LISTEN_USED[socket] = true; LISTEN_PORT[socket] = port; 0 }
}

pub fn new_socket() -> usize {
    unsafe {
        for i in 0..LISTEN_USED.len() {
            if !LISTEN_USED[i] && !CONNS.iter().any(|c| c.state != State::Free && c.listen_socket == i) {
                return i;
            }
        }
    }
    usize::MAX
}

pub fn listen(_socket: usize, _backlog: usize) -> isize { 0 }

pub fn accept(socket: usize) -> Option<usize> {
    unsafe {
        for i in 0..CONNS.len() {
            if CONNS[i].state == State::Established && CONNS[i].listen_socket == socket && !CONNS[i].accepted { CONNS[i].accepted = true; return Some(0x100 + i); }
        }
        None
    }
}

pub fn readable(handle: usize) -> bool {
    unsafe {
        if handle < 0x100 { return (0..CONNS.len()).any(|i| CONNS[i].state == State::Established && CONNS[i].listen_socket == handle && !CONNS[i].accepted); }
        let i = handle - 0x100;
        i < CONNS.len() && (CONNS[i].rx_len != 0 || CONNS[i].state == State::Closed)
    }
}

pub fn recv(handle: usize, out: &mut [u8]) -> isize {
    unsafe {
        if handle < 0x100 { return -107; }
        let i = handle - 0x100; if i >= CONNS.len() { return -9; }
        let c = &mut CONNS[i];
        if c.rx_len == 0 { return if c.state == State::Closed { 0 } else { -11 }; }
        let n = out.len().min(c.rx_len); out[..n].copy_from_slice(&c.rx[..n]); c.rx.copy_within(n..c.rx_len, 0); c.rx_len -= n; n as isize
    }
}

pub fn send(handle: usize, data: &[u8]) -> isize {
    if handle < 0x100 { return -107; }
    let id = handle - 0x100; if id >= 8 { return -9; }
    let n = data.len().min(TCP_MSS);
    unsafe { if CONNS[id].state != State::Established { return -107; } }
    send_tcp(id, 0x18, &data[..n]);
    n as isize
}

pub fn close(handle: usize) -> isize {
    unsafe { if handle >= 0x100 { let i = handle - 0x100; if i < CONNS.len() { CONNS[i].state = State::Closed; } } else if handle < LISTEN_USED.len() { LISTEN_USED[handle] = false; } 0 }
}

pub fn has_socket(handle: usize) -> bool { unsafe { handle < 0x108 && (handle < 0x100 || handle - 0x100 < CONNS.len()) } }
