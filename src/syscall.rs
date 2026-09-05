use crate::{fs, memory};
use alloc::{string::String, vec::Vec};
#[derive(Clone)]
pub enum Fd {
    Console,
    File {
        path: String,
        pos: usize,
        flags: usize,
    },
    Epoll(Vec<(usize, u32, u64)>),
    Socket(usize),
    Unix(usize, usize),
    Event(u64),
}
static mut FDS: Option<Vec<Option<Fd>>> = None;
static mut COUNT: usize = 0;
struct Channel {
    queues: [alloc::collections::VecDeque<u8>; 2],
    open: [bool; 2],
}
static mut CHANNELS: Option<Vec<Channel>> = None;
unsafe fn channels() -> &'static mut Vec<Channel> {
    CHANNELS.get_or_insert_with(Vec::new)
}

pub unsafe fn fds() -> &'static mut Vec<Option<Fd>> {
    FDS.get_or_insert_with(|| alloc::vec![Some(Fd::Console), Some(Fd::Console), Some(Fd::Console)])
}
pub unsafe fn add(f: Fd) -> usize {
    let v = fds();
    for i in 3..v.len() {
        if v[i].is_none() {
            v[i] = Some(f);
            return i;
        }
    }
    v.push(Some(f));
    v.len() - 1
}
pub unsafe fn get(fd: usize) -> Option<Fd> {
    fds().get(fd).and_then(|x| x.clone())
}
pub unsafe fn bytes(p: usize, n: usize) -> &'static [u8] {
    if n == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p as *const u8, n)
    }
}
pub unsafe fn buf(p: usize, n: usize) -> &'static mut [u8] {
    if n == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut(p as *mut u8, n)
    }
}
pub unsafe fn cstr(p: usize) -> String {
    if p == 0 {
        return String::new();
    }
    let mut n = 0;
    while *((p + n) as *const u8) != 0 && n < 4096 {
        n += 1;
    }
    String::from_utf8_lossy(bytes(p, n)).into_owned()
}
pub unsafe fn put64(p: usize, x: u64) {
    core::ptr::write_unaligned(p as *mut u64, x)
}
pub unsafe fn get64(p: usize) -> u64 {
    core::ptr::read_unaligned(p as *const u64)
}
pub unsafe fn put32(p: usize, x: u32) {
    core::ptr::write_unaligned(p as *mut u32, x)
}
pub unsafe fn get32(p: usize) -> u32 {
    core::ptr::read_unaligned(p as *const u32)
}
unsafe fn path_at(fd: usize, p: usize) -> String {
    let s = cstr(p);
    if s.starts_with('/') || fd == (-100isize as usize) {
        fs::normalize(&s)
    } else {
        match get(fd) {
            Some(Fd::File { path, .. }) => fs::normalize(&alloc::format!("{}/{}", path, s)),
            _ => fs::normalize(&s),
        }
    }
}
unsafe fn stat(path: &str, p: usize) -> isize {
    if !fs::exists(path) && !path.starts_with("/dev/") {
        return -2;
    }
    buf(p, 128).fill(0);
    put64(p, 1);
    put64(
        p + 8,
        path.bytes()
            .fold(5381u64, |h, b| h.wrapping_mul(33).wrapping_add(b as u64)),
    );
    put32(p + 16, if fs::is_dir(path) { 0o040755 } else { 0o100644 });
    put32(p + 20, 1);
    let size = fs::file_data(path).map(|f| f.len()).unwrap_or(0);
    put64(p + 48, size as u64);
    put32(p + 56, 4096);
    put64(p + 64, ((size + 511) / 512) as u64);
    put64(p + 88, 1788579000);
    0
}
unsafe fn read(fd: usize, p: usize, n: usize) -> isize {
    match get(fd) {
        Some(Fd::File { path, pos, flags }) => {
            if path == "/dev/urandom" || path == "/dev/random" {
                return random(p, n);
            }
            let Some(d) = fs::file_data(&path) else {
                return -21;
            };
            let count = n.min(d.len().saturating_sub(pos));
            buf(p, count).copy_from_slice(&d[pos.min(d.len())..pos.min(d.len()) + count]);
            fds()[fd] = Some(Fd::File {
                path,
                pos: pos + count,
                flags,
            });
            count as isize
        }
        Some(Fd::Socket(s)) => crate::net::recv(s, buf(p, n)),
        Some(Fd::Unix(id, side)) => {
            let ch = &mut channels()[id];
            if ch.queues[side].is_empty() {
                return if ch.open[1 - side] { -11 } else { 0 };
            }
            let count = n.min(ch.queues[side].len());
            for b in buf(p, count) {
                *b = ch.queues[side].pop_front().unwrap();
            }
            count as isize
        }
        Some(Fd::Event(v)) => {
            if n < 8 {
                return -22;
            }
            if v == 0 {
                return -11;
            }
            put64(p, v);
            fds()[fd] = Some(Fd::Event(0));
            8
        }
        Some(Fd::Console) => 0,
        _ => -9,
    }
}
unsafe fn write(fd: usize, p: usize, n: usize) -> isize {
    match get(fd) {
        Some(Fd::Console) => {
            for &b in bytes(p, n) {
                crate::putchar(b);
            }
            n as isize
        }
        Some(Fd::File { path, pos, flags }) => {
            if path == "/dev/null" {
                return n as isize;
            }
            fs::write(&path, pos, bytes(p, n));
            fds()[fd] = Some(Fd::File {
                path,
                pos: pos + n,
                flags,
            });
            n as isize
        }
        Some(Fd::Socket(s)) => crate::net::send(s, bytes(p, n)),
        Some(Fd::Unix(id, side)) => {
            let ch = &mut channels()[id];
            if !ch.open[1 - side] {
                return -32;
            }
            ch.queues[1 - side].extend(bytes(p, n));
            n as isize
        }
        Some(Fd::Event(v)) => {
            if n != 8 {
                return -22;
            }
            fds()[fd] = Some(Fd::Event(v.saturating_add(get64(p))));
            8
        }
        _ => -9,
    }
}
unsafe fn random(p: usize, n: usize) -> isize {
    let mut x = crate::ticks() ^ 0xa765e921ba02a941;
    for b in buf(p, n) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    n as isize
}
pub fn dispatch(n: usize, a: [usize; 6]) -> isize {
    unsafe {
        COUNT += 1;
        let ret = call(n, a);
        if cfg!(feature = "trace") && n != 22 {
            crate::println!("[sys {}] {} {:x?} -> {}", COUNT, n, a, ret);
        }
        ret
    }
}
unsafe fn call(n: usize, a: [usize; 6]) -> isize {
    let [a0, a1, a2, a3, a4, a5] = a;
    match n {
        0..=4 => crate::aio::dispatch(n, a),
        17 => {
            buf(a0, 2).copy_from_slice(b"/\0");
            a0 as isize
        }
        19 => add(Fd::Event(a0 as u64)) as isize,
        20 => add(Fd::Epoll(Vec::new())) as isize,
        21 => {
            let Some(Fd::Epoll(mut v)) = get(a0) else {
                return -9;
            };
            if a1 == 2 {
                v.retain(|x| x.0 != a2);
            } else {
                let events = get32(a3);
                let data = get64(a3 + 8);
                if let Some(x) = v.iter_mut().find(|x| x.0 == a2) {
                    *x = (a2, events, data);
                } else {
                    v.push((a2, events, data));
                }
            }
            fds()[a0] = Some(Fd::Epoll(v));
            0
        }
        22 => epoll_wait(a0, a1, a2, a3 as i32),
        23 => match get(a0) {
            Some(f) => add(f) as isize,
            None => -9,
        },
        24 => {
            if let Some(f) = get(a0) {
                while fds().len() <= a1 {
                    fds().push(None)
                }
                fds()[a1] = Some(f);
                a1 as isize
            } else {
                -9
            }
        }
        25 => match a1 {
            0 | 1030 => match get(a0) {
                Some(f) => add(f) as isize,
                None => -9,
            },
            3 => match get(a0) {
                Some(Fd::File { flags, .. }) => flags as isize,
                _ => 0,
            },
            _ => 0,
        },
        29 => {
            if a1 == 0x5421 {
                0
            } else if a1 == 0x541b {
                put32(a2, 0);
                0
            } else {
                -25
            }
        }
        34 => {
            fs::mkdir(&path_at(a0, a1));
            0
        }
        35 | 37 | 48 | 49 | 52 | 53 | 54 | 55 => 0,
        46 => {
            if let Some(Fd::File { path, .. }) = get(a0) {
                fs::VFS
                    .as_mut()
                    .unwrap()
                    .get_mut(&path)
                    .unwrap()
                    .resize(a1, 0);
                0
            } else {
                -9
            }
        }
        56 => {
            let path = path_at(a0, a1);
            if cfg!(feature = "trace") {
                crate::println!("[open] {} flags={:#x}", path, a2);
            }
            if path == "/dev/stdout" || path == "/dev/stderr" {
                return add(Fd::Console) as isize;
            }
            if !fs::exists(&path) && !path.starts_with("/dev/") {
                if a2 & 64 != 0 {
                    fs::create(&path);
                } else {
                    return -2;
                }
            }
            let pos = if a2 & 1024 != 0 {
                fs::file_data(&path).map(|f| f.len()).unwrap_or(0)
            } else {
                0
            };
            add(Fd::File {
                path,
                pos,
                flags: a2,
            }) as isize
        }
        57 => {
            if let Some(Fd::Socket(s)) = get(a0) {
                crate::net::close(s);
            }
            if let Some(Fd::Unix(id, side)) = get(a0) {
                channels()[id].open[side] = false;
            }
            if let Some(f) = fds().get_mut(a0) {
                *f = None;
                0
            } else {
                -9
            }
        }
        61 => 0,
        62 => {
            if let Some(Fd::File { path, pos, flags }) = get(a0) {
                let base = match a2 {
                    0 => 0,
                    1 => pos,
                    _ => fs::file_data(&path).map(|f| f.len()).unwrap_or(0),
                };
                let off = base.wrapping_add(a1);
                fds()[a0] = Some(Fd::File {
                    path,
                    pos: off,
                    flags,
                });
                off as isize
            } else {
                -29
            }
        }
        63 => read(a0, a1, a2),
        64 => write(a0, a1, a2),
        65 | 66 => {
            let mut total = 0;
            for i in 0..a2 {
                let p = get64(a1 + i * 16) as usize;
                let len = get64(a1 + i * 16 + 8) as usize;
                let r = if n == 65 {
                    read(a0, p, len)
                } else {
                    write(a0, p, len)
                };
                if r < 0 {
                    return if total == 0 { r } else { total };
                }
                total += r;
                if r < (len as isize) {
                    break;
                }
            }
            total
        }
        67 => {
            if let Some(Fd::File { path, .. }) = get(a0) {
                if let Some(d) = fs::file_data(&path) {
                    let start = a3.min(d.len());
                    let count = a2.min(d.len() - start);
                    buf(a1, count).copy_from_slice(&d[start..start + count]);
                    count as isize
                } else {
                    -9
                }
            } else {
                -9
            }
        }
        68 => {
            if let Some(Fd::File { path, .. }) = get(a0) {
                fs::write(&path, a3, bytes(a1, a2));
                a2 as isize
            } else {
                -9
            }
        }
        71 => {
            let Some(Fd::File { path, pos, flags }) = get(a1) else {
                return -9;
            };
            let d = fs::file_data(&path).unwrap();
            let off = if a2 != 0 { get64(a2) as usize } else { pos };
            let count = a3.min(d.len().saturating_sub(off));
            let r = write(a0, d[off..off + count].as_ptr() as usize, count);
            if r > 0 {
                if a2 != 0 {
                    put64(a2, (off + r as usize) as u64)
                } else {
                    fds()[a1] = Some(Fd::File {
                        path,
                        pos: off + r as usize,
                        flags,
                    })
                }
            }
            r
        }
        73 => {
            crate::net::poll();
            0
        }
        78 => {
            let path = path_at(a0, a1);
            let target = if path == "/proc/self/exe" {
                "/usr/sbin/nginx"
            } else {
                return -2;
            };
            let l = target.len().min(a3);
            buf(a2, l).copy_from_slice(&target.as_bytes()[..l]);
            l as isize
        }
        79 => {
            let path = path_at(a0, a1);
            stat(&path, a2)
        }
        80 => match get(a0) {
            Some(Fd::File { path, .. }) => stat(&path, a1),
            Some(_) => {
                buf(a1, 128).fill(0);
                put32(a1 + 16, 0o020666);
                0
            }
            None => -9,
        },
        93 | 94 => {
            crate::println!("[exit] nginx status={}", a0);
            crate::shutdown()
        }
        96 => 1,
        98 => {
            if a1 & 127 == 1 {
                0
            } else {
                -11
            }
        }
        99 | 100 => 0,
        101 | 115 => {
            crate::net::poll();
            0
        }
        113 => {
            let ms = crate::millis() as u64;
            let epoch = if a0 == 0 { 1788579000 } else { 0 };
            put64(a1, epoch + ms / 1000);
            put64(a1 + 8, (ms % 1000) * 1000000);
            0
        }
        114 => {
            put64(a1, 0);
            put64(a1 + 8, 100);
            0
        }
        123 => {
            buf(a2, a1).fill(0);
            buf(a2, 1)[0] = 1;
            8
        }
        124 | 129 | 130 | 131 | 132 | 134 | 135 | 136 | 137 | 138 | 140 | 144 | 146 | 147 | 149
        | 159 | 164 | 167 => 0,
        160 => {
            buf(a0, 390).fill(0);
            for (i, s) in [
                "Linux",
                "ijiege",
                "6.6.0-ijiege",
                "Rust kernel",
                "riscv64",
                "localdomain",
            ]
            .iter()
            .enumerate()
            {
                buf(a0 + i * 65, s.len()).copy_from_slice(s.as_bytes());
            }
            0
        }
        163 => {
            put64(a1, 1024);
            put64(a1 + 8, 1024);
            0
        }
        165 => {
            buf(a1, 144).fill(0);
            0
        }
        166 => 0,
        169 => {
            let ms = crate::millis() as u64;
            if a0 != 0 {
                put64(a0, 1788579000 + ms / 1000);
                put64(a0 + 8, (ms % 1000) * 1000);
            }
            0
        }
        172 | 178 => 1,
        173 | 174 | 175 | 176 | 177 => 0,
        179 => {
            buf(a0, 112).fill(0);
            put64(a0 + 32, 256 * 1024 * 1024);
            put32(a0 + 104, 1);
            0
        }
        198 => {
            let s = crate::net::socket();
            add(Fd::Socket(s)) as isize
        }
        199 => {
            if a0 != 1 {
                return -97;
            }
            let id = channels().len();
            channels().push(Channel {
                queues: Default::default(),
                open: [true, true],
            });
            let x = add(Fd::Unix(id, 0));
            let y = add(Fd::Unix(id, 1));
            put32(a3, x as u32);
            put32(a3 + 4, y as u32);
            0
        }
        200 => socket_op(a0, |s| crate::net::bind(s, bytes(a1, a2))),
        201 => socket_op(a0, |s| crate::net::listen(s, a1)),
        202 | 242 => {
            if let Some(Fd::Socket(s)) = get(a0) {
                match crate::net::accept(s) {
                    Some((new, ip, port)) => {
                        if a1 != 0 {
                            sockaddr(a1, a2, ip, port);
                        }
                        add(Fd::Socket(new)) as isize
                    }
                    None => -11,
                }
            } else {
                -9
            }
        }
        203 => -38,
        204 | 205 => {
            if let Some(Fd::Socket(s)) = get(a0) {
                let (ip, port) = crate::net::address(s, n == 205);
                sockaddr(a1, a2, ip, port);
                0
            } else {
                -9
            }
        }
        206 => write(a0, a1, a2),
        207 => read(a0, a1, a2),
        208 => 0,
        209 => {
            if a3 != 0 {
                put32(a3, 0);
                if a4 != 0 {
                    put32(a4, 4);
                }
            }
            0
        }
        210 => socket_op(a0, |s| {
            crate::net::close(s);
            0
        }),
        214 => {
            if a0 == 0 {
                return memory::BRK as isize;
            }
            if a0 > memory::BRK {
                memory::map(memory::BRK, a0 - memory::BRK);
            }
            memory::BRK = a0;
            a0 as isize
        }
        215 => 0,
        222 => {
            let p = if a3 & 16 != 0 {
                memory::map(a0, a1);
                a0
            } else {
                memory::alloc_map(a1)
            };
            if a3 & 32 == 0 {
                if let Some(Fd::File { path, .. }) = get(a4) {
                    if let Some(d) = fs::file_data(&path) {
                        let off = a5.min(d.len());
                        let count = a1.min(d.len() - off);
                        buf(p, count).copy_from_slice(&d[off..off + count]);
                    }
                }
            }
            p as isize
        }
        226 | 227 | 228 | 233 => 0,
        261 => {
            if a3 != 0 {
                put64(a3, 1024);
                put64(a3 + 8, 1024);
            }
            0
        }
        278 => random(a0, a1),
        258 | 283 | 287 | 291 | 293 => -38,
        _ => {
            crate::println!("[unimplemented] syscall {} {:x?}", n, a);
            -38
        }
    }
}
unsafe fn sockaddr(p: usize, len: usize, ip: [u8; 4], port: u16) {
    buf(p, 16).fill(0);
    buf(p, 2).copy_from_slice(&2u16.to_le_bytes());
    buf(p + 2, 2).copy_from_slice(&port.to_be_bytes());
    buf(p + 4, 4).copy_from_slice(&ip);
    if len != 0 {
        put32(len, 16);
    }
}
unsafe fn socket_op(fd: usize, f: impl FnOnce(usize) -> isize) -> isize {
    if let Some(Fd::Socket(s)) = get(fd) {
        f(s)
    } else {
        -9
    }
}
unsafe fn epoll_wait(fd: usize, out: usize, max: usize, timeout: i32) -> isize {
    let start = crate::millis();
    loop {
        crate::net::poll();
        let Some(Fd::Epoll(v)) = get(fd) else {
            return -9;
        };
        let mut count = 0;
        for (f, events, data) in v {
            let ready = match get(f) {
                Some(Fd::Socket(s)) => crate::net::ready(s),
                Some(Fd::Event(v)) => {
                    if v > 0 {
                        1
                    } else {
                        0
                    }
                }
                Some(Fd::Unix(id, side)) => {
                    let ch = &channels()[id];
                    let mut r = 4;
                    if !ch.queues[side].is_empty() {
                        r |= 1
                    }
                    if !ch.open[1 - side] {
                        r |= 0x2011
                    }
                    r
                }
                _ => 0,
            };
            let mask = ready & (events | 0x18);
            if mask != 0 {
                put32(out + count * 16, mask);
                put64(out + count * 16 + 8, data);
                count += 1;
                if count == max {
                    break;
                }
            }
        }
        if count > 0 {
            return count as isize;
        }
        if timeout >= 0 && crate::millis() - start >= timeout as i64 {
            return 0;
        }
    }
}
