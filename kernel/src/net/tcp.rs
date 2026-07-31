//! TCP engine: connection state machine, segment send/receive, retransmission.
//! Connections are identified by a stable `usize` id into a global table.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::timer;

pub const MSS: usize = 1460;
pub const MAX_WINDOW: u32 = 65535;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TcpState {
    Listen,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    Closing,
    CloseWait,
    LastAck,
    TimeWait,
    Closed,
}

#[derive(Clone)]
pub struct TcpConn {
    pub state: TcpState,
    pub saddr: u32,
    pub daddr: u32,
    pub sport: u16,
    pub dport: u16,
    pub snd_una: u32,
    pub snd_nxt: u32,
    pub rcv_nxt: u32,
    pub iss: u32,
    pub irs: u32,
    pub outbox: VecDeque<u8>,
    pub sent: Vec<(u32, Vec<u8>)>, // (seq, payload) segments awaiting ack
    pub rto: u64,                  // ms
    pub rto_deadline: u64,         // ms
    pub sock: usize,               // owning socket (for pending conns: the pre-made socket)
    pub fin_sent: bool,
    pub fin_acked: bool,
    pub peer_fin: bool,
    pub timewait_until: u64,
}

pub static mut CONNS: Vec<Option<TcpConn>> = Vec::new();
pub static mut NEXT_CONN: usize = 1;

pub fn conn_new() -> Option<usize> {
    unsafe {
        for (i, c) in CONNS.iter_mut().enumerate() {
            if c.is_none() {
                *c = Some(TcpConn {
                    state: TcpState::Closed,
                    saddr: 0,
                    daddr: 0,
                    sport: 0,
                    dport: 0,
                    snd_una: 0,
                    snd_nxt: 0,
                    rcv_nxt: 0,
                    iss: 0,
                    irs: 0,
                    outbox: VecDeque::new(),
                    sent: Vec::new(),
                    rto: 200,
                    rto_deadline: 0,
                    sock: 0,
                    fin_sent: false,
                    fin_acked: false,
                    peer_fin: false,
                    timewait_until: 0,
                });
                return Some(i);
            }
        }
        CONNS.push(Some(TcpConn {
            state: TcpState::Closed,
            saddr: 0,
            daddr: 0,
            sport: 0,
            dport: 0,
            snd_una: 0,
            snd_nxt: 0,
            rcv_nxt: 0,
            iss: 0,
            irs: 0,
            outbox: VecDeque::new(),
            sent: Vec::new(),
            rto: 200,
            rto_deadline: 0,
            sock: 0,
            fin_sent: false,
            fin_acked: false,
            peer_fin: false,
            timewait_until: 0,
        }));
        Some(CONNS.len() - 1)
    }
}

pub fn conn_free(id: usize) {
    unsafe {
        if let Some(c) = CONNS.get_mut(id) {
            if let Some(conn) = c.take() {
                drop(conn);
            }
        }
    }
}

pub fn conn(id: usize) -> Option<&'static mut TcpConn> {
    unsafe {
        let c = CONNS.get_mut(id)?;
        c.as_mut()
    }
}

fn rand_seq() -> u32 {
    // simple PRNG from time + counter
    let t = timer::rdtime() as u32;
    t.wrapping_mul(2654435761).wrapping_add(0x9e3779b9)
}

// ---------- sending ----------

pub struct TcpSeg {
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub payload: Vec<u8>,
    pub mss: bool,
}

pub const FLAG_FIN: u8 = 0x01;
pub const FLAG_SYN: u8 = 0x02;
pub const FLAG_RST: u8 = 0x04;
pub const FLAG_ACK: u8 = 0x10;

/// Build and transmit a TCP segment (via IP layer).
pub fn send_seg(conn: &TcpConn, seg: &TcpSeg) {
    let mut hdr = [0u8; 20];
    hdr[0..2].copy_from_slice(&conn.sport.to_be_bytes());
    hdr[2..4].copy_from_slice(&conn.dport.to_be_bytes());
    hdr[4..8].copy_from_slice(&seg.seq.to_be_bytes());
    hdr[8..12].copy_from_slice(&seg.ack.to_be_bytes());
    let dataoff = if seg.mss { 24usize } else { 20usize };
    hdr[12] = ((dataoff / 4) as u8) << 4;
    hdr[13] = seg.flags;
    hdr[14..16].copy_from_slice(&(MAX_WINDOW as u16).to_be_bytes());
    // checksum zero for now
    let mut pkt = Vec::with_capacity(dataoff + seg.payload.len());
    pkt.extend_from_slice(&hdr);
    if seg.mss {
        pkt.extend_from_slice(&[2, 4, (MSS >> 8) as u8, MSS as u8]);
    }
    pkt.extend_from_slice(&seg.payload);
    let sum = tcp_checksum(conn.saddr, conn.daddr, &pkt);
    pkt[16..18].copy_from_slice(&sum.to_be_bytes());
    crate::net::ip_tx(6, conn.daddr, &pkt);
}

fn tcp_checksum(saddr: u32, daddr: u32, tcp: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut add = |b: &[u8]| {
        let mut i = 0;
        while i + 1 < b.len() {
            sum += ((b[i] as u32) << 8) | b[i + 1] as u32;
            i += 2;
        }
        if i < b.len() {
            sum += (b[i] as u32) << 8;
        }
    };
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&saddr.to_be_bytes());
    pseudo[4..8].copy_from_slice(&daddr.to_be_bytes());
    pseudo[8] = 0;
    pseudo[9] = 6;
    pseudo[10..12].copy_from_slice(&((tcp.len() as u16).to_be_bytes()));
    add(&pseudo);
    add(tcp);
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    (!sum as u16)
}

/// Pump queued app data into the network.
pub fn tcp_output(id: usize) {
    let (saddr, daddr, sport, dport, window) = {
        let c = conn(id).unwrap();
        (
            c.saddr,
            c.daddr,
            c.sport,
            c.dport,
            MAX_WINDOW as u32 - (c.snd_nxt.wrapping_sub(c.snd_una)),
        )
    };
    let _ = (saddr, daddr, sport, dport);
    loop {
        let (seq, ack) = {
            let c = conn(id).unwrap();
            if c.outbox.is_empty() {
                break;
            }
            if c.snd_nxt.wrapping_sub(c.snd_una) >= window {
                break;
            }
            (c.snd_nxt, c.rcv_nxt)
        };
        let n = {
            let c = conn(id).unwrap();
            core::cmp::min(MSS, core::cmp::min(c.outbox.len(), (window - c.snd_nxt.wrapping_sub(c.snd_una)) as usize))
        };
        if n == 0 {
            break;
        }
        let payload: Vec<u8> = {
            let c = conn(id).unwrap();
            c.outbox.drain(..n).collect()
        };
        {
            let c = conn(id).unwrap();
            let seg = TcpSeg {
                seq,
                ack,
                flags: FLAG_ACK,
                payload: payload.clone(),
                mss: false,
            };
            send_seg(c, &seg);
            c.sent.push((seq, payload));
            c.snd_nxt = c.snd_nxt.wrapping_add(n as u32);
            c.rto = 200;
            c.rto_deadline = timer::now_ms() + c.rto;
        }
    }
}

pub fn send_ack(c: &TcpConn) {
    let seg = TcpSeg {
        seq: c.snd_nxt,
        ack: c.rcv_nxt,
        flags: FLAG_ACK,
        payload: Vec::new(),
        mss: false,
    };
    send_seg(c, &seg);
}

pub fn send_syn_ack(c: &TcpConn) {
    let seg = TcpSeg {
        seq: c.iss,
        ack: c.irs.wrapping_add(1),
        flags: FLAG_SYN | FLAG_ACK,
        payload: Vec::new(),
        mss: true,
    };
    send_seg(c, &seg);
    c.snd_nxt = c.iss.wrapping_add(1);
    c.snd_una = c.iss;
    // SYN occupies a sequence number
    c.sent.push((c.iss, Vec::new()));
    c.rto = 200;
    c.rto_deadline = timer::now_ms() + c.rto;
}

pub fn send_fin(id: usize) {
    let (seq, ack) = {
        let c = conn(id).unwrap();
        (c.snd_nxt, c.rcv_nxt)
    };
    let c = conn(id).unwrap();
    let seg = TcpSeg {
        seq,
        ack,
        flags: FLAG_ACK | FLAG_FIN,
        payload: Vec::new(),
        mss: false,
    };
    send_seg(c, &seg);
    c.sent.push((seq, Vec::new()));
    c.snd_nxt = c.snd_nxt.wrapping_add(1);
    c.fin_sent = true;
    c.rto = 200;
    c.rto_deadline = timer::now_ms() + c.rto;
    if c.state == TcpState::Established {
        c.state = TcpState::FinWait1;
    } else if c.state == TcpState::CloseWait {
        c.state = TcpState::LastAck;
    }
}

pub fn send_rst(c: &TcpConn, seq: u32) {
    let seg = TcpSeg {
        seq,
        ack: 0,
        flags: FLAG_RST,
        payload: Vec::new(),
        mss: false,
    };
    send_seg(c, &seg);
}

// ---------- receive ----------

/// Called from IP layer with the full TCP segment (header + payload).
pub fn tcp_input(src: u32, sport: u16, dst_port: u16, seg: &[u8]) {
    if seg.len() < 20 {
        return;
    }
    let seq = u32::from_be_bytes(seg[4..8].try_into().unwrap());
    let ack = u32::from_be_bytes(seg[8..12].try_into().unwrap());
    let flags = seg[13];
    crate::kprintln!(
        "[tcp] input src={}.{}.{}.{}:{} -> :{} seq={} ack={} flags={:#x} len={}",
        src >> 24, (src >> 16) & 0xff, (src >> 8) & 0xff, src & 0xff, sport, dst_port, seq, ack, flags, seg.len()
    );
    let dataoff = ((seg[12] >> 4) as usize) * 4;
    if dataoff > seg.len() {
        return;
    }
    let payload = &seg[dataoff..];

    let syn = flags & FLAG_SYN != 0;
    let ackf = flags & FLAG_ACK != 0;
    let fin = flags & FLAG_FIN != 0;
    let rst = flags & FLAG_RST != 0;

    // find connection: by (dport, src, sport) among established/connecting conns
    let conn_id = find_conn(dst_port, src, sport);
    let conn_id = match conn_id {
        Some(id) => Some(id),
        None => {
            if syn && !ackf {
                // new connection to a listener
                crate::net::listener_on_syn(dst_port, src, sport, seq)
            } else {
                None
            }
        }
    };
    let id = match conn_id {
        Some(id) => id,
        None => {
            // no such connection: send RST (only if not RST itself)
            if !rst {
                crate::kprintln!("[tcp] NO CONN for dport={} src={} sport={} syn={} ackf={} -> RST", dst_port, src, sport, syn, ackf);
                crate::net::send_rst_to(dst_port, src, sport, seq.wrapping_add(payload.len() as u32));
            }
            return;
        }
    };

    match conn(id).unwrap().state {
        TcpState::SynReceived => {
            if ackf {
                let c = conn(id).unwrap();
                crate::kprintln!(
                    "[tcp] SynReceived ack={} snd_nxt={} iss={} -> {}",
                    ack, c.snd_nxt, c.iss, if ack == c.snd_nxt { "ESTABLISH" } else { "mismatch" }
                );
                if ack == c.snd_nxt {
                    c.state = TcpState::Established;
                    c.rcv_nxt = seq.wrapping_add(payload.len() as u32);
                    // remove SYN from sent list
                    c.sent.retain(|(s, _)| *s != c.iss);
                    c.snd_una = ack;
                    c.rto_deadline = 0;
                    crate::net::conn_established(id);
                    if fin {
                        handle_data_and_fin(id, seq, payload, true);
                    } else if !payload.is_empty() {
                        handle_data_and_fin(id, seq, payload, false);
                    } else {
                        send_ack(c);
                    }
                }
            }
        }
        TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {
            if rst {
                crate::net::conn_reset(id);
                return;
            }
            // ACK processing
            if ackf {
                let c = conn(id).unwrap();
                // ack must not be beyond snd_nxt
                if ack.wrapping_sub(c.snd_nxt) > 0x8000_0000 {
                    return; // bogus ack
                }
                if ack.wrapping_sub(c.snd_una) > 0 {
                    c.snd_una = ack;
                    // remove fully-acked segments (keep those with end > ack)
                    let a = ack;
                    c.sent.retain(|(s, d)| {
                        let end = s.wrapping_add(d.len() as u32);
                        a.wrapping_sub(end) > 0x8000_0000
                    });
                    if c.sent.is_empty() {
                        c.rto_deadline = 0;
                    } else {
                        c.rto = 200;
                        c.rto_deadline = timer::now_ms() + c.rto;
                    }
                    // FIN acked?
                    if c.fin_sent && !c.fin_acked && c.sent.is_empty() {
                        c.fin_acked = true;
                        if c.state == TcpState::FinWait1 {
                            c.state = TcpState::FinWait2;
                        } else if c.state == TcpState::LastAck {
                            c.state = TcpState::Closed;
                            conn_free(id);
                            return;
                        }
                    }
                    // push more data
                    tcp_output(id);
                }
            }
            let c = conn(id).unwrap();
            // data + fin handling (guard against duplicate data)
            if !payload.is_empty() || fin {
                let s = seq;
                let rcv = c.rcv_nxt;
                if s == rcv {
                    if !payload.is_empty() {
                        crate::net::sock_rx_push(c.sock, payload);
                        c.rcv_nxt = c.rcv_nxt.wrapping_add(payload.len() as u32);
                    }
                    if fin {
                        c.peer_fin = true;
                        c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                        crate::net::sock_peer_fin(c.sock);
                        if c.state == TcpState::FinWait2 {
                            // both sides done
                            c.state = TcpState::TimeWait;
                            c.timewait_until = timer::now_ms() + 2000;
                            crate::timer_wheel::set_timer(c.timewait_until, 0, crate::timer_wheel::TimerKind::Net);
                        } else if c.state == TcpState::FinWait1 {
                            c.state = TcpState::Closing;
                        } else {
                            c.state = TcpState::CloseWait;
                        }
                    }
                    send_ack(c);
                } else if s.wrapping_sub(rcv) > 0x8000_0000 {
                    // old/duplicate segment: ack current
                    send_ack(c);
                } else {
                    // future segment (gap): send ack, don't buffer
                    send_ack(c);
                }
            }
        }
        TcpState::CloseWait | TcpState::LastAck | TcpState::Closing => {
            if ackf && !crate::tcp::conn(id).unwrap().sent.is_empty() {
                // process acks
                let c = conn(id).unwrap();
                if ack.wrapping_sub(c.snd_una) > 0 {
                    c.snd_una = ack;
                    c.sent.retain(|(s, d)| {
                        let end = s.wrapping_add(d.len() as u32);
                        ack.wrapping_sub(end) > 0x8000_0000
                    });
                    if c.sent.is_empty() {
                        c.rto_deadline = 0;
                        if c.state == TcpState::LastAck {
                            c.state = TcpState::Closed;
                            conn_free(id);
                            return;
                        }
                        if c.state == TcpState::Closing {
                            c.state = TcpState::TimeWait;
                            c.timewait_until = timer::now_ms() + 2000;
                            crate::timer_wheel::set_timer(c.timewait_until, 0, crate::timer_wheel::TimerKind::Net);
                        }
                    }
                }
            }
            let c = conn(id).unwrap();
            if fin && !c.peer_fin {
                c.peer_fin = true;
                c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                crate::net::sock_peer_fin(c.sock);
                if c.state == TcpState::Closing {
                    c.state = TcpState::TimeWait;
                    c.timewait_until = timer::now_ms() + 2000;
                    crate::timer_wheel::set_timer(c.timewait_until, 0, crate::timer_wheel::TimerKind::Net);
                }
                send_ack(c);
            } else if fin {
                send_ack(c);
            }
        }
        TcpState::TimeWait => {
            let c = conn(id).unwrap();
            if fin {
                send_ack(c);
            }
        }
        _ => {}
    }
}

fn handle_data_and_fin(id: usize, seq: u32, payload: &[u8], fin: bool) {
    let c = conn(id).unwrap();
    let _ = c;
    let mut p = payload;
    let mut s = seq;
    // In SynReceived transition we already set rcv_nxt
    if !p.is_empty() {
        crate::net::sock_rx_push(crate::tcp::conn(id).unwrap().sock, p);
        crate::tcp::conn(id).unwrap().rcv_nxt = s.wrapping_add(p.len() as u32);
        s = s.wrapping_add(p.len() as u32);
    }
    if fin {
        crate::tcp::conn(id).unwrap().peer_fin = true;
        crate::net::sock_peer_fin(crate::tcp::conn(id).unwrap().sock);
        crate::tcp::conn(id).unwrap().rcv_nxt = crate::tcp::conn(id).unwrap().rcv_nxt.wrapping_add(1);
    }
    let c = crate::tcp::conn(id).unwrap();
    send_ack(c);
    if c.peer_fin && c.state == TcpState::Established {
        c.state = TcpState::CloseWait;
    }
}

fn find_conn(dport: u16, src: u32, sport: u16) -> Option<usize> {
    unsafe {
        for (i, c) in CONNS.iter().enumerate() {
            if let Some(c) = c {
                // include SynReceived so the handshake-completing ACK matches;
                // a retransmitted SYN hitting a SynReceived conn is ignored in
                // the state machine (no ACK flag), so this is safe.
                if c.dport == dport && c.saddr == src && c.sport == sport && c.state != TcpState::Closed && c.state != TcpState::Listen && c.state != TcpState::TimeWait {
                    return Some(i);
                }
            }
        }
        None
    }
}

/// Periodic: retransmit timed-out segments, free timed-out TimeWait conns.
pub fn net_tick() {
    let now = timer::now_ms();
    unsafe {
        let ids: Vec<usize> = CONNS
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_some())
            .map(|(i, _)| i)
            .collect();
        for id in ids {
            let conn = CONNS.get_mut(id).unwrap();
            let c = match conn.as_mut() {
                Some(c) => c,
                None => continue,
            };
            if c.state == TcpState::TimeWait {
                if now >= c.timewait_until {
                    drop(c);
                    conn.take();
                    continue;
                }
                continue;
            }
            if !c.sent.is_empty() && c.rto_deadline != 0 && now >= c.rto_deadline {
                // retransmit all unacked segments
                let segs: Vec<(u32, Vec<u8>)> = c.sent.clone();
                for (s, d) in segs {
                    let seg = TcpSeg {
                        seq: s,
                        ack: c.rcv_nxt,
                        flags: FLAG_ACK | if d.is_empty() && s == c.snd_nxt.wrapping_sub(1) && c.fin_sent && !c.fin_acked { FLAG_FIN } else { 0 },
                        payload: d,
                        mss: false,
                    };
                    send_seg(c, &seg);
                }
                c.rto = core::cmp::min(c.rto * 2, 4000);
                c.rto_deadline = now + c.rto;
            }
        }
    }
}
