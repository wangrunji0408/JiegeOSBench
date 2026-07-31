//! Network stack: sockets, ARP, IPv4, and the glue to virtio-net.
//! Guest IP 10.0.2.15 (QEMU user networking / slirp), gateway 10.0.2.2.

pub mod tcp;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::console::kprintln;
use crate::timer;

pub const OUR_IP: u32 = 0x0a00_020f; // 10.0.2.15
pub const GATEWAY: u32 = 0x0a00_0202; // 10.0.2.2

pub static mut OUR_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x57];

// ---------- sockets ----------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SockState {
    Free,
    Unbound,
    Listening,
    Connected,
    Closed,
}

#[derive(Clone)]
pub struct Socket {
    pub state: SockState,
    pub domain: i32,
    pub sock_type: i32,
    pub nonblock: bool,
    pub reuseaddr: bool,
    pub error: i32,
    pub rx: VecDeque<u8>,
    pub peer_fin: bool,
    pub local_fin: bool,
    pub local_ip: u32,
    pub local_port: u16,
    pub peer_ip: u32,
    pub peer_port: u16,
    pub conn: Option<usize>,      // tcp conn id (AF_INET stream)
    pub backlog: Vec<usize>,      // established conns awaiting accept (listener)
    pub syn_queue: Vec<usize>,    // conns in SynReceived
    pub peer_sock: Option<usize>, // unix socketpair peer
    pub last_activity: u64,
}

pub static mut SOCKS: Vec<Option<Socket>> = Vec::new();
pub static mut NEXT_SOCK: usize = 1;

fn sock_new() -> usize {
    unsafe {
        for (i, s) in SOCKS.iter_mut().enumerate() {
            if s.is_none() {
                *s = Some(Socket {
                    state: SockState::Unbound,
                    domain: 0,
                    sock_type: 0,
                    nonblock: false,
                    reuseaddr: false,
                    error: 0,
                    rx: VecDeque::new(),
                    peer_fin: false,
                    local_fin: false,
                    local_ip: 0,
                    local_port: 0,
                    peer_ip: 0,
                    peer_port: 0,
                    conn: None,
                    backlog: Vec::new(),
                    syn_queue: Vec::new(),
                    peer_sock: None,
                    last_activity: 0,
                });
                return i;
            }
        }
        SOCKS.push(Some(Socket {
            state: SockState::Unbound,
            domain: 0,
            sock_type: 0,
            nonblock: false,
            reuseaddr: false,
            error: 0,
            rx: VecDeque::new(),
            peer_fin: false,
            local_fin: false,
            local_ip: 0,
            local_port: 0,
            peer_ip: 0,
            peer_port: 0,
            conn: None,
            backlog: Vec::new(),
            syn_queue: Vec::new(),
            peer_sock: None,
            last_activity: 0,
        }));
        SOCKS.len() - 1
    }
}

pub fn sock(id: usize) -> Option<&'static mut Socket> {
    unsafe { SOCKS.get_mut(id)?.as_mut() }
}

pub fn sock_init() {}

// ---------- ARP ----------

static mut ARP_CACHE: Vec<(u32, [u8; 6])> = Vec::new();
static mut ARP_PENDING: Vec<(u32, Vec<u8>)> = Vec::new();
static mut ARP_LAST_REQ: u64 = 0;

fn arp_lookup(ip: u32) -> Option<[u8; 6]> {
    unsafe {
        for (i, m) in ARP_CACHE.iter() {
            if *i == ip {
                return Some(*m);
            }
        }
        None
    }
}

fn arp_add(ip: u32, mac: [u8; 6]) {
    unsafe {
        ARP_CACHE.retain(|(i, _)| *i != ip);
        ARP_CACHE.push((ip, mac));
        if ARP_CACHE.len() > 32 {
            ARP_CACHE.remove(0);
        }
    }
}

pub fn arp_request(ip: u32) {
    unsafe {
        ARP_LAST_REQ = timer::now_ms();
        let mut pkt = Vec::with_capacity(42);
        pkt.extend_from_slice(&[0xff; 6]);
        pkt.extend_from_slice(&OUR_MAC);
        pkt.extend_from_slice(&[0x08, 0x06]);
        // arp header
        pkt.extend_from_slice(&[0x00, 0x01]); // htype ethernet
        pkt.extend_from_slice(&[0x08, 0x00]); // ptype ip
        pkt.push(6);
        pkt.push(4);
        pkt.extend_from_slice(&[0x00, 0x01]); // request
        pkt.extend_from_slice(&OUR_MAC);
        pkt.extend_from_slice(&OUR_IP.to_be_bytes());
        pkt.extend_from_slice(&[0x00; 6]);
        pkt.extend_from_slice(&ip.to_be_bytes());
        crate::virtio::net_tx(&pkt);
    }
}

fn arp_rx(p: &[u8]) {
    if p.len() < 28 {
        return;
    }
    let oper = u16::from_be_bytes([p[6], p[7]]);
    let sha: [u8; 6] = p[8..14].try_into().unwrap();
    let spa = u32::from_be_bytes(p[14..18].try_into().unwrap());
    let tpa = u32::from_be_bytes(p[24..28].try_into().unwrap());
    if oper == 1 && tpa == OUR_IP {
        // reply
        let mut pkt = Vec::with_capacity(42);
        pkt.extend_from_slice(&sha);
        pkt.extend_from_slice(&OUR_MAC);
        pkt.extend_from_slice(&[0x08, 0x06]);
        pkt.extend_from_slice(&[0x00, 0x01]);
        pkt.extend_from_slice(&[0x08, 0x00]);
        pkt.push(6);
        pkt.push(4);
        pkt.extend_from_slice(&[0x00, 0x02]); // reply
        pkt.extend_from_slice(&OUR_MAC);
        pkt.extend_from_slice(&OUR_IP.to_be_bytes());
        pkt.extend_from_slice(&sha);
        pkt.extend_from_slice(&spa.to_be_bytes());
        crate::virtio::net_tx(&pkt);
    } else if oper == 2 {
        arp_add(spa, sha);
        // flush pending frames
        unsafe {
            let mut i = 0;
            while i < ARP_PENDING.len() {
                if ARP_PENDING[i].0 == spa {
                    let (_, frame) = ARP_PENDING.remove(i);
                    crate::virtio::net_tx(&frame);
                } else {
                    i += 1;
                }
            }
        }
    }
}

// ---------- IPv4 ----------

pub fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

static mut IP_ID: u16 = 0x1234;

pub fn ip_tx(proto: u8, dst: u32, payload: &[u8]) {
    // resolve MAC
    let mac = match arp_lookup(dst) {
        Some(m) => m,
        None => {
            // queue and request
            let mut frame = Vec::with_capacity(14 + 20 + payload.len());
            frame.extend_from_slice(&[0; 14]); // placeholder
            frame.extend_from_slice(&[0x45, 0x00]);
            let total = (20 + payload.len()) as u16;
            frame.extend_from_slice(&total.to_be_bytes());
            unsafe {
                IP_ID = IP_ID.wrapping_add(1);
            }
            frame.extend_from_slice(&unsafe { IP_ID }.to_be_bytes());
            frame.extend_from_slice(&[0x00, 0x00]); // flags/frag
            frame.push(64); // ttl
            frame.push(proto);
            frame.extend_from_slice(&[0x00, 0x00]); // checksum placeholder
            frame.extend_from_slice(&OUR_IP.to_be_bytes());
            frame.extend_from_slice(&dst.to_be_bytes());
            frame.extend_from_slice(payload);
            let csum = ip_checksum(&frame[14..34]);
            frame[24..26].copy_from_slice(&csum.to_be_bytes());
            // fill ethernet header
            unsafe {
                ARP_PENDING.push((dst, frame));
                if timer::now_ms().saturating_sub(ARP_LAST_REQ) > 500 {
                    arp_request(dst);
                }
            }
            return;
        }
    };
    let mut frame = Vec::with_capacity(14 + 20 + payload.len());
    frame.extend_from_slice(&mac);
    frame.extend_from_slice(&OUR_MAC);
    frame.extend_from_slice(&[0x08, 0x00]);
    frame.extend_from_slice(&[0x45, 0x00]);
    let total = (20 + payload.len()) as u16;
    frame.extend_from_slice(&total.to_be_bytes());
    unsafe {
        IP_ID = IP_ID.wrapping_add(1);
    }
    frame.extend_from_slice(&unsafe { IP_ID }.to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.push(64);
    frame.push(proto);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&OUR_IP.to_be_bytes());
    frame.extend_from_slice(&dst.to_be_bytes());
    frame.extend_from_slice(payload);
    let csum = ip_checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&csum.to_be_bytes());
    crate::virtio::net_tx(&frame);
}

fn ip_rx(p: &[u8]) {
    if p.len() < 20 {
        return;
    }
    let version = p[0] >> 4;
    if version != 4 {
        return;
    }
    let ihl = ((p[0] & 0xf) as usize) * 4;
    if p.len() < ihl {
        return;
    }
    let total = u16::from_be_bytes([p[2], p[3]]) as usize;
    if total > p.len() {
        return;
    }
    let dst = u32::from_be_bytes(p[16..20].try_into().unwrap());
    if dst != OUR_IP && dst != 0xffff_ffff {
        return;
    }
    let csum = ip_checksum(&p[..ihl]);
    if csum != 0 {
        return;
    }
    let src = u32::from_be_bytes(p[12..16].try_into().unwrap());
    crate::kprintln!(
        "[net] ip_rx src={}.{}.{}.{} dst={}.{}.{}.{} proto={} total={}",
        src >> 24, (src >> 16) & 0xff, (src >> 8) & 0xff, src & 0xff,
        dst >> 24, (dst >> 16) & 0xff, (dst >> 8) & 0xff, dst & 0xff,
        p[9], total
    );
    let proto = p[9];
    let payload = &p[ihl..total];
    match proto {
        6 => {
            if payload.len() < 20 {
                return;
            }
            let src = u32::from_be_bytes(p[12..16].try_into().unwrap());
            let sport = u16::from_be_bytes([payload[0], payload[1]]);
            let dport = u16::from_be_bytes([payload[2], payload[3]]);
            tcp::tcp_input(src, sport, dport, payload);
        }
        1 => {
            // ICMP echo reply
            if payload.len() >= 8 && payload[0] == 8 {
                let mut reply = payload.to_vec();
                reply[0] = 0;
                reply[2..4].copy_from_slice(&[0, 0]);
                let csum = ip_checksum(&reply);
                reply[2..4].copy_from_slice(&csum.to_be_bytes());
                ip_tx(1, u32::from_be_bytes(p[12..16].try_into().unwrap()), &reply);
            }
        }
        _ => {}
    }
}

// ---------- ethernet rx ----------

pub fn net_rx_frame(frame: &[u8]) {
    if frame.len() < 14 {
        return;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    let payload = &frame[14..];
    match ethertype {
        0x0806 => arp_rx(payload),
        0x0800 => ip_rx(payload),
        _ => {}
    }
}

// ---------- tcp glue ----------

pub fn listener_on_syn(dport: u16, src: u32, sport: u16, seq: u32) -> Option<usize> {
    // find listening socket on dport
    let listener = find_listener(dport)?;
    // create conn + socket
    let sock_id = sock_new();
    let conn_id = tcp::conn_new()?;
    {
        let c = tcp::conn(conn_id).unwrap();
        c.state = tcp::TcpState::SynReceived;
        c.saddr = src;
        c.daddr = OUR_IP;
        c.sport = sport;
        c.dport = dport;
        c.irs = seq;
        c.iss = crate::tcp::rand_seq();
        c.snd_una = c.iss;
        c.snd_nxt = c.iss;
        c.rcv_nxt = seq.wrapping_add(1);
        c.sock = sock_id;
        c.sent.clear();
        tcp::send_syn_ack(c);
    }
    {
        let s = sock(sock_id).unwrap();
        s.state = SockState::Connected;
        s.domain = 2;
        s.sock_type = 1;
        s.local_ip = OUR_IP;
        s.local_port = dport;
        s.peer_ip = src;
        s.peer_port = sport;
        s.conn = Some(conn_id);
    }
    {
        let l = sock(listener).unwrap();
        l.syn_queue.push(conn_id);
    }
    Some(conn_id)
}

pub fn find_listener(dport: u16) -> Option<usize> {
    unsafe {
        for (i, s) in SOCKS.iter().enumerate() {
            if let Some(s) = s {
                if s.state == SockState::Listening && s.local_port == dport {
                    return Some(i);
                }
            }
        }
        None
    }
}

/// Called by tcp engine when handshake completes.
pub fn conn_established(conn_id: usize) {
    let (sock_id, listener) = {
        let c = tcp::conn(conn_id).unwrap();
        let sock_id = c.sock;
        // find listener by local port
        let listener = find_listener(c.sport).unwrap();
        (sock_id, listener)
    };
    {
        let s = sock(sock_id).unwrap();
        s.state = SockState::Connected;
    }
    {
        let l = sock(listener).unwrap();
        l.syn_queue.retain(|&x| x != conn_id);
        l.backlog.push(conn_id);
        // mark listener readable: wake epoll waiters
        sock_state_changed(listener);
    }
    let _ = sock_id;
}

pub fn conn_reset(conn_id: usize) {
    let (sock_id, dport, sport, src) = {
        let c = tcp::conn(conn_id).unwrap();
        (c.sock, c.dport, c.sport, c.saddr)
    };
    let _ = (dport, sport, src);
    {
        let s = sock(sock_id).unwrap();
        s.error = 104; // ECONNRESET
        s.state = SockState::Closed;
    }
    tcp::conn_free(conn_id);
    sock_state_changed(sock_id);
}

pub fn send_rst_to(dport: u16, src: u32, sport: u16, seq: u32) {
    let mut tmp = tcp::TcpConn {
        state: tcp::TcpState::Closed,
        saddr: OUR_IP,
        daddr: src,
        sport: dport,
        dport: sport,
        snd_una: 0,
        snd_nxt: 0,
        rcv_nxt: 0,
        iss: 0,
        irs: 0,
        outbox: VecDeque::new(),
        sent: Vec::new(),
        rto: 0,
        rto_deadline: 0,
        sock: 0,
        fin_sent: false,
        fin_acked: false,
        peer_fin: false,
        timewait_until: 0,
    };
    let seg = tcp::TcpSeg {
        seq,
        ack: 0,
        flags: tcp::FLAG_RST,
        payload: Vec::new(),
        mss: false,
    };
    tcp::send_seg(&mut tmp, &seg);
}

/// Push received data into a socket's rx buffer; wake waiters.
pub fn sock_rx_push(sock_id: usize, data: &[u8]) {
    let s = sock(sock_id).unwrap();
    if s.rx.len() + data.len() > 512 * 1024 {
        // drop excess (flow control is crude)
        return;
    }
    s.rx.extend_from_slice(data);
    s.last_activity = timer::now_ms();
    drop(s);
    sock_state_changed(sock_id);
    wake_sock(sock_id);
}

pub fn sock_peer_fin(sock_id: usize) {
    let s = sock(sock_id).unwrap();
    s.peer_fin = true;
    drop(s);
    sock_state_changed(sock_id);
    wake_sock(sock_id);
}

/// Wake tasks blocked on this socket's wchan.
pub fn wake_sock(sock_id: usize) {
    crate::task::wake_wchan(sock_id);
}

/// Notify epoll machinery that socket state may have changed.
pub fn sock_state_changed(_sock_id: usize) {
    crate::epoll::wake_all_epoll();
}

// ---------- socket API (called from syscalls) ----------

pub fn sock_create(domain: i32, sock_type: i32) -> Result<usize, i32> {
    let id = sock_new();
    let s = sock(id).unwrap();
    s.domain = domain;
    s.sock_type = sock_type;
    Ok(id)
}

pub fn sock_bind(id: usize, ip: u32, port: u16) -> Result<(), i32> {
    let s = sock(id).unwrap();
    if port == 0 {
        return Err(-22); // EINVAL (nginx always uses fixed port)
    }
    // check conflict
    if !s.reuseaddr {
        unsafe {
            for (i, other) in SOCKS.iter().enumerate() {
                if let Some(o) = other {
                    if i != id && o.state != SockState::Free && o.state != SockState::Closed
                        && o.local_port == port && o.local_ip == ip
                    {
                        return Err(-98); // EADDRINUSE
                    }
                }
            }
        }
    }
    s.local_ip = ip;
    s.local_port = port;
    Ok(())
}

pub fn sock_listen(id: usize, backlog: i32) -> Result<(), i32> {
    let s = sock(id).unwrap();
    if s.local_port == 0 {
        return Err(-22);
    }
    s.state = SockState::Listening;
    let _ = backlog;
    Ok(())
}

/// Accept one pending connection; returns (new sock id, peer ip, peer port).
pub fn sock_accept(id: usize, nonblock: bool) -> Result<(usize, u32, u16), i32> {
    loop {
        let has_pending = {
            let s = sock(id).unwrap();
            !s.backlog.is_empty()
        };
        if has_pending {
            let conn_id = {
                let s = sock(id).unwrap();
                s.backlog.remove(0)
            };
            let new_sock = {
                let c = tcp::conn(conn_id).unwrap();
                c.sock
            };
            {
                let s = sock(new_sock).unwrap();
                s.nonblock = nonblock;
            }
            // clear listener readability hint (level-triggered scan handles it)
            return Ok((new_sock, {
                let s = sock(new_sock).unwrap();
                (s.peer_ip, s.peer_port)
            }));
        }
        let s = sock(id).unwrap();
        if s.error != 0 {
            let e = s.error;
            s.error = 0;
            return Err(-e);
        }
        if s.nonblock {
            return Err(-11); // EAGAIN
        }
        // block until a connection arrives
        crate::task::block_on(id);
    }
}

pub fn sock_read(id: usize, buf: &mut [u8]) -> Result<usize, i32> {
    loop {
        let (n, fin, err) = {
            let s = sock(id).unwrap();
            if s.error != 0 {
                (0, false, Some(s.error))
            } else if !s.rx.is_empty() {
                let n = core::cmp::min(buf.len(), s.rx.len());
                for i in 0..n {
                    buf[i] = s.rx.pop_front().unwrap();
                }
                (n, false, None)
            } else if s.peer_fin {
                (0, true, None)
            } else {
                (0, false, None)
            }
        };
        if let Some(e) = err {
            let s = sock(id).unwrap();
            s.error = 0;
            return Err(-e);
        }
        if fin {
            return Ok(0);
        }
        if n > 0 {
            return Ok(n);
        }
        // no data
        let s = sock(id).unwrap();
        if s.nonblock {
            return Err(-11);
        }
        crate::task::block_on(id);
    }
}

pub fn sock_write(id: usize, buf: &[u8]) -> Result<usize, i32> {
    loop {
        // unix socketpair: write to peer
        let peer = {
            let s = sock(id).unwrap();
            s.peer_sock
        };
        if let Some(peer_id) = peer {
            let s = sock(peer_id).unwrap();
            if s.state == SockState::Closed {
                return Err(-32); // EPIPE
            }
            if s.rx.len() + buf.len() > 512 * 1024 {
                let s2 = sock(id).unwrap();
                if s2.nonblock {
                    return Err(-11);
                }
                crate::task::block_on(id);
                continue;
            }
            s.rx.extend_from_slice(buf);
            drop(s);
            sock_state_changed(peer_id);
            wake_sock(peer_id);
            return Ok(buf.len());
        }
        // TCP
        let conn_id = {
            let s = sock(id).unwrap();
            match s.conn {
                Some(c) => c,
                None => return Err(-57), // ENOTCONN
            }
        };
        let (closed, peer_fin, outbox_len, space) = {
            let c = tcp::conn(conn_id).unwrap();
            (
                c.state == tcp::TcpState::Closed || c.state == tcp::TcpState::TimeWait,
                c.peer_fin,
                c.outbox.len(),
                c.snd_nxt.wrapping_sub(c.snd_una) < tcp::MAX_WINDOW,
            )
        };
        if closed || peer_fin {
            // send SIGPIPE default: handled by syscall layer; here just EPIPE
            return Err(-32);
        }
        if outbox_len + buf.len() > 256 * 1024 || !space {
            let s = sock(id).unwrap();
            if s.nonblock {
                return Err(-11);
            }
            crate::task::block_on(id);
            continue;
        }
        {
            let c = tcp::conn(conn_id).unwrap();
            c.outbox.extend_from_slice(buf);
        }
        tcp::tcp_output(conn_id);
        return Ok(buf.len());
    }
}

pub fn sock_shutdown(id: usize, how: i32) -> Result<(), i32> {
    let conn_id = {
        let s = sock(id).unwrap();
        s.conn
    };
    if let Some(cid) = conn_id {
        let state = {
            let c = tcp::conn(cid).unwrap();
            c.state
        };
        if how == 1 || how == 2 {
            // SHUT_WR / SHUT_RDWR: send FIN
            if state == tcp::TcpState::Established || state == tcp::TcpState::CloseWait {
                tcp::send_fin(cid);
            }
        }
    }
    let s = sock(id).unwrap();
    let _ = s;
    Ok(())
}

pub fn sock_close(id: usize) {
    let (conn_id, peer) = {
        let s = sock(id).unwrap();
        (s.conn, s.peer_sock)
    };
    if let Some(cid) = conn_id {
        let state = {
            let c = tcp::conn(cid).unwrap();
            c.state
        };
        match state {
            tcp::TcpState::Established | tcp::TcpState::SynReceived => {
                tcp::send_fin(cid);
            }
            tcp::TcpState::CloseWait => {
                tcp::send_fin(cid);
            }
            tcp::TcpState::FinWait1 | tcp::TcpState::FinWait2 => {}
            _ => {
                tcp::conn_free(cid);
            }
        }
    }
    if let Some(p) = peer {
        // unix pair: mark closed; peer sees fin
        let s = sock(id).unwrap();
        s.state = SockState::Closed;
        let ps = sock(p).unwrap();
        ps.peer_fin = true;
        ps.error = 104; // ECONNRESET on read
        drop(ps);
        sock_state_changed(p);
        wake_sock(p);
    }
    unsafe {
        SOCKS[id] = None;
    }
    sock_state_changed(id);
}

pub fn sock_readable(id: usize) -> bool {
    let s = match sock(id) {
        Some(s) => s,
        None => return false,
    };
    if s.error != 0 {
        return true;
    }
    match s.state {
        SockState::Listening => !s.backlog.is_empty(),
        _ => {
            !s.rx.is_empty()
                || s.peer_fin
                || (s.peer_sock.is_some() && {
                    let p = sock(s.peer_sock.unwrap()).unwrap();
                    p.state == SockState::Closed
                })
        }
    }
}

pub fn sock_writable(id: usize) -> bool {
    let s = match sock(id) {
        Some(s) => s,
        None => return false,
    };
    if s.peer_sock.is_some() {
        let p = sock(s.peer_sock.unwrap()).unwrap();
        return p.state != SockState::Closed && p.rx.len() < 512 * 1024;
    }
    match s.conn {
        Some(cid) => {
            let c = tcp::conn(cid).unwrap();
            c.state != tcp::TcpState::Closed
                && c.state != tcp::TcpState::TimeWait
                && !c.peer_fin
                && c.outbox.len() < 256 * 1024
                && c.snd_nxt.wrapping_sub(c.snd_una) < tcp::MAX_WINDOW
        }
        None => true,
    }
}

pub fn sock_getsockname(id: usize) -> (u32, u16) {
    let s = sock(id).unwrap();
    (s.local_ip, s.local_port)
}

pub fn sock_getpeername(id: usize) -> (u32, u16) {
    let s = sock(id).unwrap();
    (s.peer_ip, s.peer_port)
}

pub fn sock_setsockopt(id: usize, level: i32, opt: i32, val: &[u8]) -> Result<(), i32> {
    let _ = (level, opt);
    match opt {
        1 => {
            // SO_REUSEADDR
            let s = sock(id).unwrap();
            s.reuseaddr = val.len() >= 4 && i32::from_le_bytes(val[..4].try_into().unwrap()) != 0;
        }
        4 => {
            // SO_REUSEPORT (noop)
        }
        7 => {
            // SO_LINGER (noop)
        }
        8 => {
            // SO_KEEPALIVE (noop)
        }
        2 => {
            // SO_RCVBUF
        }
        3 => {
            // SO_SNDBUF
        }
        _ => {}
    }
    Ok(())
}

pub fn sock_getsockopt(id: usize, level: i32, opt: i32, out: &mut [u8]) -> Result<usize, i32> {
    let _ = level;
    match opt {
        4 => {
            // SO_ERROR
            let e = {
                let s = sock(id).unwrap();
                s.error
            };
            let s = sock(id).unwrap();
            s.error = 0;
            out[..4].copy_from_slice(&e.to_le_bytes());
            Ok(4)
        }
        3 => {
            // SO_TYPE
            let t = {
                let s = sock(id).unwrap();
                s.sock_type
            };
            out[..4].copy_from_slice(&t.to_le_bytes());
            Ok(4)
        }
        1 => {
            // SO_REUSEADDR
            let v = {
                let s = sock(id).unwrap();
                s.reuseaddr as i32
            };
            out[..4].copy_from_slice(&v.to_le_bytes());
            Ok(4)
        }
        _ => Err(-92), // ENOPROTOOPT
    }
}

pub fn sock_socketpair(sock_type: i32) -> Result<(usize, usize), i32> {
    let a = sock_new();
    let b = sock_new();
    {
        let s = sock(a).unwrap();
        s.domain = 1; // AF_UNIX
        s.sock_type = sock_type;
        s.state = SockState::Connected;
        s.peer_sock = Some(b);
    }
    {
        let s = sock(b).unwrap();
        s.domain = 1;
        s.sock_type = sock_type;
        s.state = SockState::Connected;
        s.peer_sock = Some(a);
    }
    Ok((a, b))
}

/// Debug dump of socket table.
pub fn dump_socks() {
    unsafe {
        for (i, s) in SOCKS.iter().enumerate() {
            if let Some(s) = s {
                kprintln!(
                    "  sock {} state={:?} lport={} rx={} conn={:?} back={}",
                    i, s.state, s.local_port, s.rx.len(), s.conn, s.backlog.len()
                );
            }
        }
    }
}
