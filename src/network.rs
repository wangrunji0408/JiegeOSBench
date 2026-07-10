use core::{
    arch::asm,
    ptr::{read_volatile, write_volatile},
};

use crate::memory;

const QSIZE: usize = 8;
const VIRTQ_DESC_F_WRITE: u16 = 2;
// QEMU's virtio-net header includes the two-byte num_buffers field.
const NET_HEADER: usize = 12;

static mut DEVICE: usize = 0;
static mut MAC: [u8; 6] = [0; 6];
static mut RX_DESC: usize = 0;
static mut RX_AVAIL: usize = 0;
static mut RX_USED: usize = 0;
static mut RX_BUFFERS: [usize; QSIZE] = [0; QSIZE];
static mut RX_LAST_USED: u16 = 0;
static mut RX_AVAIL_INDEX: u16 = QSIZE as u16;
static mut TX_DESC: usize = 0;
static mut TX_AVAIL: usize = 0;
static mut TX_USED: usize = 0;
static mut TX_BUFFERS: [usize; QSIZE] = [0; QSIZE];
static mut TX_AVAIL_INDEX: u16 = 0;

static mut PEER_MAC: [u8; 6] = [0; 6];
static mut PEER_IP: [u8; 4] = [0; 4];
static mut PEER_PORT: u16 = 0;
static mut CLIENT_SEQUENCE: u32 = 0;
static mut SERVER_SEQUENCE: u32 = 0x1234_0000;
static mut TCP_STATE: u8 = 0;
static mut REQUEST: [u8; 4096] = [0; 4096];
static mut REQUEST_LENGTH: usize = 0;
static mut ACCEPT_READY: bool = false;
static mut ACCEPTED: bool = false;
static mut PEER_FIN_PENDING: bool = false;
static mut LISTEN_EVENT_DATA: u64 = 0;
static mut CONNECTION_EVENT_DATA: u64 = 0;

#[inline]
unsafe fn reg32(base: usize, offset: usize) -> u32 {
    unsafe { read_volatile((base + offset) as *const u32) }
}
#[inline]
unsafe fn set32(base: usize, offset: usize, value: u32) {
    unsafe { write_volatile((base + offset) as *mut u32, value) }
}

pub fn init() {
    let mut found = 0;
    for index in 0..8 {
        let base = 0x1000_1000 + index * 0x1000;
        let magic = unsafe { reg32(base, 0) };
        let device_id = unsafe { reg32(base, 8) };
        if magic == 0x7472_6976 && device_id == 1 {
            found = base;
            break;
        }
    }
    if found == 0 {
        crate::println!("virtio-net: not present");
        return;
    }
    unsafe {
        DEVICE = found;
        set32(found, 0x70, 0);
        set32(found, 0x70, 1);
        set32(found, 0x70, 1 | 2);
        // Negotiate only VIRTIO_F_VERSION_1 (bit 32); the fixed MAC is in config.
        set32(found, 0x24, 0);
        set32(found, 0x20, 0);
        set32(found, 0x24, 1);
        set32(found, 0x20, 1);
        set32(found, 0x70, 1 | 2 | 8);
        assert!(reg32(found, 0x70) & 8 != 0, "virtio features rejected");
        for index in 0..6 {
            MAC[index] = read_volatile((found + 0x100 + index) as *const u8);
        }
        setup_queue(0, true);
        setup_queue(1, false);
        asm!("fence iorw, iorw");
        set32(found, 0x70, 1 | 2 | 8 | 4);
        set32(found, 0x50, 0);
        crate::println!(
            "virtio-net @ {:#x}, mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            found,
            MAC[0],
            MAC[1],
            MAC[2],
            MAC[3],
            MAC[4],
            MAC[5]
        );
        set32(found, 0x30, 0);
        crate::println!(
            "virtio status={:#x} rx-ready={} rx-max={}",
            reg32(found, 0x70),
            reg32(found, 0x44),
            reg32(found, 0x34)
        );
    }
    announce();
}

fn announce() {
    let mut frame = [0u8; 42];
    frame[..6].fill(0xff);
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(MAC).cast::<u8>(),
            frame[6..12].as_mut_ptr(),
            6,
        );
    }
    frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    frame[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(MAC).cast::<u8>(),
            frame[22..28].as_mut_ptr(),
            6,
        );
    }
    frame[28..32].copy_from_slice(&[10, 0, 2, 15]);
    frame[38..42].copy_from_slice(&[10, 0, 2, 2]);
    transmit(&frame);
}

unsafe fn setup_queue(queue: usize, receive: bool) {
    let base = unsafe { DEVICE };
    unsafe { set32(base, 0x30, queue as u32) };
    let maximum = unsafe { reg32(base, 0x34) } as usize;
    assert!(maximum >= QSIZE, "virtio queue too small");
    unsafe { set32(base, 0x38, QSIZE as u32) };
    let descriptor = memory::alloc_frame();
    let available = memory::alloc_frame();
    let used = memory::alloc_frame();
    unsafe {
        set32(base, 0x80, descriptor as u32);
        set32(base, 0x84, (descriptor >> 32) as u32);
        set32(base, 0x90, available as u32);
        set32(base, 0x94, (available >> 32) as u32);
        set32(base, 0xa0, used as u32);
        set32(base, 0xa4, (used >> 32) as u32);
        set32(base, 0x44, 1);
    }
    if receive {
        unsafe {
            RX_DESC = descriptor;
            RX_AVAIL = available;
            RX_USED = used;
            for index in 0..QSIZE {
                let buffer = memory::alloc_frame();
                RX_BUFFERS[index] = buffer;
                write_descriptor(
                    descriptor,
                    index,
                    buffer,
                    memory::PAGE_SIZE as u32,
                    VIRTQ_DESC_F_WRITE,
                );
                write_u16(available + 4 + index * 2, index as u16);
            }
            write_u16(available + 2, QSIZE as u16);
        }
    } else {
        unsafe {
            TX_DESC = descriptor;
            TX_AVAIL = available;
            TX_USED = used;
            for index in 0..QSIZE {
                TX_BUFFERS[index] = memory::alloc_frame();
            }
        }
    }
}

unsafe fn write_descriptor(table: usize, index: usize, address: usize, length: u32, flags: u16) {
    let offset = table + index * 16;
    unsafe {
        write_volatile(offset as *mut u64, address as u64);
        write_volatile((offset + 8) as *mut u32, length);
        write_volatile((offset + 12) as *mut u16, flags);
        write_volatile((offset + 14) as *mut u16, 0);
    }
}
unsafe fn read_u16(address: usize) -> u16 {
    unsafe { read_volatile(address as *const u16) }
}
unsafe fn write_u16(address: usize, value: u16) {
    unsafe { write_volatile(address as *mut u16, value) }
}

pub fn poll() {
    if unsafe { DEVICE } == 0 {
        return;
    }
    loop {
        let device_index = unsafe { read_u16(RX_USED + 2) };
        if device_index == unsafe { RX_LAST_USED } {
            break;
        }
        let slot = unsafe { RX_LAST_USED } as usize % QSIZE;
        let element = unsafe { RX_USED } + 4 + slot * 8;
        let id = unsafe { read_volatile(element as *const u32) } as usize;
        let length = unsafe { read_volatile((element + 4) as *const u32) } as usize;
        if id < QSIZE && length > NET_HEADER {
            let packet = unsafe { RX_BUFFERS[id] } + NET_HEADER;
            process_frame(packet, length - NET_HEADER);
        }
        unsafe {
            RX_LAST_USED = RX_LAST_USED.wrapping_add(1);
            let avail_slot = RX_AVAIL_INDEX as usize % QSIZE;
            write_u16(RX_AVAIL + 4 + avail_slot * 2, id as u16);
            asm!("fence w, w");
            RX_AVAIL_INDEX = RX_AVAIL_INDEX.wrapping_add(1);
            write_u16(RX_AVAIL + 2, RX_AVAIL_INDEX);
            set32(DEVICE, 0x50, 0);
        }
    }
}

fn process_frame(packet: usize, length: usize) {
    if length < 14 {
        return;
    }
    let ethertype = unsafe { u16::from_be(read_volatile((packet + 12) as *const u16)) };
    match ethertype {
        0x0806 => process_arp(packet, length),
        0x0800 => process_ipv4(packet, length),
        _ => {}
    }
}

fn process_arp(packet: usize, length: usize) {
    if length < 42 {
        return;
    }
    let operation = unsafe { u16::from_be(read_volatile((packet + 20) as *const u16)) };
    let target_ip = unsafe { core::slice::from_raw_parts((packet + 38) as *const u8, 4) };
    if operation != 1 || target_ip != [10, 0, 2, 15] {
        return;
    }
    let mut frame = [0u8; 42];
    unsafe {
        core::ptr::copy_nonoverlapping((packet + 6) as *const u8, frame.as_mut_ptr(), 6);
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(MAC).cast::<u8>(),
            frame[6..12].as_mut_ptr(),
            6,
        );
        frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        frame[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(MAC).cast::<u8>(),
            frame[22..28].as_mut_ptr(),
            6,
        );
        frame[28..32].copy_from_slice(&[10, 0, 2, 15]);
        core::ptr::copy_nonoverlapping((packet + 22) as *const u8, frame[32..38].as_mut_ptr(), 6);
        core::ptr::copy_nonoverlapping((packet + 28) as *const u8, frame[38..42].as_mut_ptr(), 4);
    }
    transmit(&frame);
}

fn process_ipv4(packet: usize, length: usize) {
    if length < 54 {
        return;
    }
    let ip = packet + 14;
    let ihl = unsafe { (read_volatile(ip as *const u8) & 0xf) as usize * 4 };
    if ihl < 20 || length < 14 + ihl + 20 {
        return;
    }
    let protocol = unsafe { read_volatile((ip + 9) as *const u8) };
    let destination = unsafe { core::slice::from_raw_parts((ip + 16) as *const u8, 4) };
    if protocol != 6 || destination != [10, 0, 2, 15] {
        return;
    }
    let tcp = ip + ihl;
    let destination_port = unsafe { u16::from_be(read_volatile((tcp + 2) as *const u16)) };
    if destination_port != 80 {
        return;
    }
    let source_port = unsafe { u16::from_be(read_volatile(tcp as *const u16)) };
    let sequence = unsafe { u32::from_be(read_volatile((tcp + 4) as *const u32)) };
    let header_length = unsafe { (read_volatile((tcp + 12) as *const u8) >> 4) as usize * 4 };
    let flags = unsafe { read_volatile((tcp + 13) as *const u8) };
    let total = unsafe { u16::from_be(read_volatile((ip + 2) as *const u16)) } as usize;
    let payload_length = total.saturating_sub(ihl + header_length);
    if flags & 4 != 0 {
        reset_connection();
        return;
    }
    if flags & 2 != 0 {
        if unsafe { ACCEPTED } {
            // The previous connection has not yet been released by nginx. The
            // peer will retransmit this SYN after its FIN has been delivered.
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                (packet + 6) as *const u8,
                core::ptr::addr_of_mut!(PEER_MAC).cast::<u8>(),
                6,
            );
            core::ptr::copy_nonoverlapping(
                (ip + 12) as *const u8,
                core::ptr::addr_of_mut!(PEER_IP).cast::<u8>(),
                4,
            );
            PEER_PORT = source_port;
            CLIENT_SEQUENCE = sequence.wrapping_add(1);
            TCP_STATE = 1;
            REQUEST_LENGTH = 0;
            ACCEPT_READY = false;
            PEER_FIN_PENDING = false;
        }
        send_tcp(2 | 0x10, &[]);
        unsafe {
            SERVER_SEQUENCE = SERVER_SEQUENCE.wrapping_add(1);
        }
        return;
    }
    if unsafe { TCP_STATE } == 1 && flags & 0x10 != 0 {
        unsafe {
            TCP_STATE = 2;
            ACCEPT_READY = true;
        }
    }
    if payload_length > 0 && header_length >= 20 {
        let count = payload_length.min(4096);
        unsafe {
            core::ptr::copy_nonoverlapping(
                (tcp + header_length) as *const u8,
                core::ptr::addr_of_mut!(REQUEST).cast::<u8>(),
                count,
            );
            REQUEST_LENGTH = count;
            CLIENT_SEQUENCE = sequence.wrapping_add(payload_length as u32);
            ACCEPT_READY = true;
        }
    }
    if flags & 1 != 0 {
        unsafe {
            CLIENT_SEQUENCE = sequence.wrapping_add(payload_length as u32).wrapping_add(1);
            PEER_FIN_PENDING = true;
        }
    }
    if payload_length > 0 || flags & 1 != 0 {
        send_tcp(0x10, &[]);
    }
}

fn checksum(data: &[u8], initial: u32) -> u16 {
    let mut sum = initial;
    let mut index = 0;
    while index + 1 < data.len() {
        sum += u16::from_be_bytes([data[index], data[index + 1]]) as u32;
        index += 2;
    }
    if index < data.len() {
        sum += (data[index] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn send_tcp(flags: u8, payload: &[u8]) {
    let mut frame = [0u8; 1514];
    let length = 14 + 20 + 20 + payload.len();
    if length > frame.len() {
        return;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(PEER_MAC).cast::<u8>(),
            frame[..6].as_mut_ptr(),
            6,
        );
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(MAC).cast::<u8>(),
            frame[6..12].as_mut_ptr(),
            6,
        );
        frame[26..30].copy_from_slice(&[10, 0, 2, 15]);
        core::ptr::copy_nonoverlapping(
            core::ptr::addr_of!(PEER_IP).cast::<u8>(),
            frame[30..34].as_mut_ptr(),
            4,
        );
        frame[34..36].copy_from_slice(&80u16.to_be_bytes());
        frame[36..38].copy_from_slice(&PEER_PORT.to_be_bytes());
        frame[38..42].copy_from_slice(&SERVER_SEQUENCE.to_be_bytes());
        frame[42..46].copy_from_slice(&CLIENT_SEQUENCE.to_be_bytes());
    }
    frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&((40 + payload.len()) as u16).to_be_bytes());
    frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    frame[22] = 64;
    frame[23] = 6;
    frame[46] = 5 << 4;
    frame[47] = flags;
    frame[48..50].copy_from_slice(&64240u16.to_be_bytes());
    frame[54..length].copy_from_slice(payload);
    let ip_checksum = checksum(&frame[14..34], 0);
    frame[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    let tcp_len = (20 + payload.len()) as u16;
    let pseudo = u16::from_be_bytes([frame[26], frame[27]]) as u32
        + u16::from_be_bytes([frame[28], frame[29]]) as u32
        + u16::from_be_bytes([frame[30], frame[31]]) as u32
        + u16::from_be_bytes([frame[32], frame[33]]) as u32
        + 6
        + tcp_len as u32;
    let tcp_checksum = checksum(&frame[34..length], pseudo);
    frame[50..52].copy_from_slice(&tcp_checksum.to_be_bytes());
    transmit(&frame[..length]);
    unsafe {
        SERVER_SEQUENCE = SERVER_SEQUENCE.wrapping_add(payload.len() as u32);
    }
}

fn transmit(frame: &[u8]) {
    if unsafe { DEVICE } == 0 || frame.len() + NET_HEADER > memory::PAGE_SIZE {
        return;
    }
    unsafe {
        while TX_AVAIL_INDEX.wrapping_sub(read_u16(TX_USED + 2)) as usize >= QSIZE {
            core::hint::spin_loop();
        }
        let slot = TX_AVAIL_INDEX as usize % QSIZE;
        let buffer = TX_BUFFERS[slot];
        core::ptr::write_bytes(buffer as *mut u8, 0, NET_HEADER);
        core::ptr::copy_nonoverlapping(
            frame.as_ptr(),
            (buffer + NET_HEADER) as *mut u8,
            frame.len(),
        );
        write_descriptor(TX_DESC, slot, buffer, (NET_HEADER + frame.len()) as u32, 0);
        write_u16(TX_AVAIL + 4 + slot * 2, slot as u16);
        asm!("fence w, w");
        TX_AVAIL_INDEX = TX_AVAIL_INDEX.wrapping_add(1);
        write_u16(TX_AVAIL + 2, TX_AVAIL_INDEX);
        set32(DEVICE, 0x50, 1);
    }
}

pub fn epoll_ctl(fd: usize, event: usize) -> isize {
    if event == 0 {
        return 0;
    }
    let mut bytes = [0u8; 8];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let Some(value) = memory::read_user_byte(event + 8 + index) else {
            return -14;
        };
        *byte = value;
    }
    let data = u64::from_le_bytes(bytes);
    unsafe {
        if fd == 101 {
            LISTEN_EVENT_DATA = data;
        }
        if fd == 102 {
            CONNECTION_EVENT_DATA = data;
        }
    }
    0
}

fn write_epoll_event(output: usize, data: u64) -> isize {
    for (index, byte) in 1u32.to_le_bytes().iter().enumerate() {
        if !memory::write_user_byte(output + index, *byte) {
            return -14;
        }
    }
    for (index, byte) in data.to_le_bytes().iter().enumerate() {
        if !memory::write_user_byte(output + 8 + index, *byte) {
            return -14;
        }
    }
    1
}

pub fn epoll_wait(output: usize) -> isize {
    loop {
        poll();
        unsafe {
            if ACCEPT_READY && !ACCEPTED {
                return write_epoll_event(output, LISTEN_EVENT_DATA);
            }
            if ACCEPTED && REQUEST_LENGTH > 0 && CONNECTION_EVENT_DATA != 0 {
                return write_epoll_event(output, CONNECTION_EVENT_DATA);
            }
            if ACCEPTED && PEER_FIN_PENDING && CONNECTION_EVENT_DATA != 0 {
                return write_epoll_event(output, CONNECTION_EVENT_DATA);
            }
        }
        core::hint::spin_loop();
    }
}

pub fn accept(address: usize, length_pointer: usize) -> isize {
    unsafe {
        if !ACCEPT_READY {
            return -11;
        }
        ACCEPTED = true;
    }
    if address != 0 {
        let family = 2u16.to_le_bytes();
        memory::write_user_byte(address, family[0]);
        memory::write_user_byte(address + 1, family[1]);
        let port = unsafe { PEER_PORT }.to_be_bytes();
        memory::write_user_byte(address + 2, port[0]);
        memory::write_user_byte(address + 3, port[1]);
        for index in 0..4 {
            memory::write_user_byte(address + 4 + index, unsafe { PEER_IP[index] });
        }
        memory::zero_user(address + 8, 8);
        if length_pointer != 0 {
            for (index, byte) in 16u32.to_le_bytes().iter().enumerate() {
                memory::write_user_byte(length_pointer + index, *byte);
            }
        }
    }
    102
}

pub fn socket_name(address: usize, length_pointer: usize, peer: bool) -> isize {
    if address == 0 {
        return -14;
    }
    let family = 2u16.to_le_bytes();
    memory::write_user_byte(address, family[0]);
    memory::write_user_byte(address + 1, family[1]);
    let port = if peer { unsafe { PEER_PORT } } else { 80 }.to_be_bytes();
    memory::write_user_byte(address + 2, port[0]);
    memory::write_user_byte(address + 3, port[1]);
    let ip = if peer {
        unsafe { PEER_IP }
    } else {
        [10, 0, 2, 15]
    };
    for index in 0..4 {
        memory::write_user_byte(address + 4 + index, ip[index]);
    }
    memory::zero_user(address + 8, 8);
    if length_pointer != 0 {
        for (index, byte) in 16u32.to_le_bytes().iter().enumerate() {
            memory::write_user_byte(length_pointer + index, *byte);
        }
    }
    0
}

pub fn receive(output: usize, length: usize) -> isize {
    unsafe {
        if REQUEST_LENGTH == 0 {
            if PEER_FIN_PENDING {
                return 0;
            }
            return -11;
        }
        let count = length.min(REQUEST_LENGTH);
        for index in 0..count {
            if !memory::write_user_byte(output + index, REQUEST[index]) {
                return -14;
            }
        }
        REQUEST_LENGTH = 0;
        count as isize
    }
}

pub fn close_connection() -> isize {
    unsafe {
        if ACCEPTED && TCP_STATE >= 2 {
            send_tcp(0x11, &[]);
            SERVER_SEQUENCE = SERVER_SEQUENCE.wrapping_add(1);
        }
    }
    reset_connection();
    0
}

fn reset_connection() {
    unsafe {
        TCP_STATE = 0;
        REQUEST_LENGTH = 0;
        ACCEPT_READY = false;
        ACCEPTED = false;
        PEER_FIN_PENDING = false;
        CONNECTION_EVENT_DATA = 0;
    }
}

pub fn send(user_buffer: usize, length: usize) -> isize {
    let mut offset = 0;
    while offset < length {
        let count = (length - offset).min(1400);
        let mut data = [0u8; 1400];
        for index in 0..count {
            let Some(byte) = memory::read_user_byte(user_buffer + offset + index) else {
                return -14;
            };
            data[index] = byte;
        }
        send_tcp(0x18, &data[..count]);
        offset += count;
    }
    length as isize
}

pub fn send_bytes(data: &[u8]) -> isize {
    for chunk in data.chunks(1400) {
        send_tcp(0x18, chunk);
    }
    data.len() as isize
}
