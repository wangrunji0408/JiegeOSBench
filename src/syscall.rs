use core::{ptr, slice, str};

use crate::{arch, console, net, vfs};

const ENOENT: isize = -2;
const EBADF: isize = -9;
const EAGAIN: isize = -11;
const ENOMEM: isize = -12;
const EINVAL: isize = -22;
const ENOSYS: isize = -38;
const ENOTTY: isize = -25;
const ENOTSOCK: isize = -88;
const ENOTCONN: isize = -107;
const ECHILD: isize = -10;

#[derive(Clone, Copy)]
struct EpollItem {
    fd: i32,
    events: u32,
    data: u64,
}

#[derive(Clone, Copy)]
enum Fd {
    Empty,
    Stdout,
    Stdin,
    File {
        index: usize,
        pos: usize,
    },
    Socket {
        handle: usize,
    },
    Epoll {
        items: [EpollItem; 16],
        count: usize,
    },
}

const EMPTY_ITEM: EpollItem = EpollItem {
    fd: -1,
    events: 0,
    data: 0,
};
static mut FDS: [Fd; 128] = [Fd::Empty; 128];
static mut NEXT_MAP: usize = 0x9000_0000;
static mut CURRENT_BRK: usize = 0x8e00_0000;

pub fn init() {
    unsafe {
        FDS[0] = Fd::Stdin;
        FDS[1] = Fd::Stdout;
        FDS[2] = Fd::Stdout;
    }
}

fn alloc_fd(value: Fd) -> isize {
    unsafe {
        for i in 3..FDS.len() {
            if matches!(FDS[i], Fd::Empty) {
                FDS[i] = value;
                return i as isize;
            }
        }
    }
    ENOMEM
}

unsafe fn get_fd(fd: usize) -> Option<Fd> {
    if fd >= FDS.len() {
        None
    } else {
        let value = FDS[fd];
        if matches!(value, Fd::Empty) {
            None
        } else {
            Some(value)
        }
    }
}

unsafe fn set_fd(fd: usize, value: Fd) -> bool {
    if fd >= FDS.len() {
        false
    } else {
        FDS[fd] = value;
        true
    }
}

unsafe fn user_bytes<'a>(ptr_value: usize, len: usize) -> &'a [u8] {
    slice::from_raw_parts(ptr_value as *const u8, len)
}

unsafe fn user_bytes_mut<'a>(ptr_value: usize, len: usize) -> &'a mut [u8] {
    slice::from_raw_parts_mut(ptr_value as *mut u8, len)
}

unsafe fn user_cstr(ptr_value: usize) -> Option<&'static str> {
    if ptr_value == 0 {
        return None;
    }
    let mut len = 0;
    while len < 4096 && ptr::read((ptr_value + len) as *const u8) != 0 {
        len += 1;
    }
    if len == 4096 {
        return None;
    }
    str::from_utf8(user_bytes(ptr_value, len)).ok()
}

fn stat_file(out: usize, size: usize, mode: u32, inode: u64) -> isize {
    unsafe {
        let s = user_bytes_mut(out, 128);
        s.fill(0);
        s[0..8].copy_from_slice(&1u64.to_ne_bytes());
        s[8..16].copy_from_slice(&inode.to_ne_bytes());
        s[16..20].copy_from_slice(&mode.to_ne_bytes());
        s[20..24].copy_from_slice(&1u32.to_ne_bytes());
        s[48..56].copy_from_slice(&(size as i64).to_ne_bytes());
        s[56..60].copy_from_slice(&(4096u32).to_ne_bytes());
        s[64..72].copy_from_slice(&((size.div_ceil(4096)) as i64).to_ne_bytes());
        s[72..80].copy_from_slice(&1u64.to_ne_bytes());
        s[80..88].copy_from_slice(&1u64.to_ne_bytes());
        s[88..96].copy_from_slice(&1u64.to_ne_bytes());
    }
    0
}

fn fill_sockaddr(out: usize, len_ptr: usize, ip: [u8; 4], port: u16) {
    unsafe {
        let s = user_bytes_mut(out, 16);
        s.fill(0);
        s[0..2].copy_from_slice(&2u16.to_ne_bytes());
        s[2..4].copy_from_slice(&port.to_be_bytes());
        s[4..8].copy_from_slice(&ip);
        if len_ptr != 0 {
            user_bytes_mut(len_ptr, 4).copy_from_slice(&16u32.to_ne_bytes());
        }
    }
}

fn open_path(path: &str, _flags: usize) -> isize {
    if path == "/dev/null" {
        return alloc_fd(Fd::File {
            index: usize::MAX,
            pos: 0,
        });
    }
    if path == "/dev/zero" {
        return alloc_fd(Fd::File {
            index: usize::MAX - 1,
            pos: 0,
        });
    }
    if path == "/dev/stderr" || path == "/dev/stdout" {
        return alloc_fd(Fd::Stdout);
    }
    if let Some(index) = vfs::lookup(path) {
        return alloc_fd(Fd::File { index, pos: 0 });
    }
    ENOENT
}

fn read_file(fd: usize, out: &mut [u8]) -> isize {
    unsafe {
        let value = match get_fd(fd) {
            Some(v) => v,
            None => return EBADF,
        };
        match value {
            Fd::Stdin => 0,
            Fd::File {
                index: usize::MAX, ..
            } => 0,
            Fd::File { index, .. } if index == usize::MAX - 1 => {
                out.fill(0);
                out.len() as isize
            }
            Fd::File { index, pos } => {
                let data = match vfs::data(index) {
                    Some(d) => d,
                    None => return EBADF,
                };
                if pos >= data.len() {
                    return 0;
                }
                let n = out.len().min(data.len() - pos);
                out[..n].copy_from_slice(&data[pos..pos + n]);
                let _ = set_fd(
                    fd,
                    Fd::File {
                        index,
                        pos: pos + n,
                    },
                );
                n as isize
            }
            _ => EBADF,
        }
    }
}

fn read_file_at(index: usize, offset: usize, out: &mut [u8]) -> isize {
    if index == usize::MAX {
        return out.len() as isize;
    }
    if index == usize::MAX - 1 {
        out.fill(0);
        return out.len() as isize;
    }
    let data = match vfs::data(index) {
        Some(d) => d,
        None => return EBADF,
    };
    if offset >= data.len() {
        return 0;
    }
    let n = out.len().min(data.len() - offset);
    out[..n].copy_from_slice(&data[offset..offset + n]);
    n as isize
}

fn write_fd(fd: usize, data: &[u8]) -> isize {
    unsafe {
        let value = match get_fd(fd) {
            Some(v) => v,
            None => return EBADF,
        };
        match value {
            Fd::Stdout => {
                console::write_bytes(data);
                data.len() as isize
            }
            Fd::File {
                index: usize::MAX, ..
            } => data.len() as isize,
            Fd::Socket { handle } => net::send(handle, data),
            _ => EBADF,
        }
    }
}

fn epoll_ready(item: EpollItem) -> u32 {
    unsafe {
        match get_fd(item.fd as usize) {
            Some(Fd::Socket { handle }) => {
                let mut r = 0;
                if item.events & 1 != 0 && net::readable(handle) {
                    r |= 1;
                }
                if item.events & 4 != 0 && net::has_socket(handle) {
                    r |= 4;
                }
                r
            }
            Some(Fd::File { .. }) => {
                if item.events & 1 != 0 {
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
}

fn epoll_wait(fd: usize, out: usize, maxevents: usize, timeout: isize) -> isize {
    let start = arch::time();
    loop {
        net::poll();
        let mut n = 0;
        unsafe {
            let value = match get_fd(fd) {
                Some(v) => v,
                None => return EBADF,
            };
            let items = match value {
                Fd::Epoll { items, count } => (items, count),
                _ => return EINVAL,
            };
            for item in items.0.iter().take(items.1) {
                let ready = epoll_ready(*item);
                if ready == 0 {
                    continue;
                }
                if n >= maxevents {
                    break;
                }
                let dst = user_bytes_mut(out + n * 16, 16);
                dst.fill(0);
                dst[0..4].copy_from_slice(&ready.to_ne_bytes());
                dst[8..16].copy_from_slice(&item.data.to_ne_bytes());
                n += 1;
            }
        }
        if n != 0 || timeout == 0 {
            return n as isize;
        }
        if timeout > 0
            && arch::time().wrapping_sub(start) >= (timeout as u64).saturating_mul(10_000)
        {
            return 0;
        }
    }
}

pub fn dispatch(tf: &mut arch::TrapFrame) {
    let nr = tf.regs[17];
    let a = |n| tf.arg(n);
    let result = match nr {
        17 => {
            // getcwd
            let path = b"/\0";
            unsafe {
                user_bytes_mut(a(0), a(1))[..path.len()].copy_from_slice(path);
            }
            a(1) as isize
        }
        20 => alloc_fd(Fd::Epoll {
            items: [EMPTY_ITEM; 16],
            count: 0,
        }),
        21 => {
            // epoll_ctl
            let ep = a(0);
            let op = a(1);
            let fd = a(2) as i32;
            unsafe {
                let value = match get_fd(ep) {
                    Some(Fd::Epoll {
                        mut items,
                        mut count,
                    }) => {
                        let mut ev = EMPTY_ITEM;
                        if a(3) != 0 {
                            let p = user_bytes(a(3), 16);
                            ev.events = u32::from_ne_bytes([p[0], p[1], p[2], p[3]]);
                            ev.data = u64::from_ne_bytes([
                                p[8], p[9], p[10], p[11], p[12], p[13], p[14], p[15],
                            ]);
                        }
                        ev.fd = fd;
                        if op == 1 {
                            if count < items.len() {
                                items[count] = ev;
                                count += 1;
                            }
                        } else if op == 2 {
                            for x in &mut items[..count] {
                                if x.fd == fd {
                                    *x = ev;
                                }
                            }
                        } else if op == 3 {
                            for i in 0..count {
                                if items[i].fd == fd {
                                    items[i] = items[count - 1];
                                    count -= 1;
                                    break;
                                }
                            }
                        }
                        Some(Fd::Epoll { items, count })
                    }
                    _ => None,
                };
                match value {
                    Some(v) => {
                        set_fd(ep, v);
                        0
                    }
                    None => EBADF,
                }
            }
        }
        22 => epoll_wait(a(0), a(1), a(2), a(3) as isize),
        23 => {
            // dup
            unsafe {
                match get_fd(a(0)) {
                    Some(v) => alloc_fd(v),
                    None => EBADF,
                }
            }
        }
        24 => {
            // dup3
            if a(0) == a(1) {
                EINVAL
            } else {
                unsafe {
                    match get_fd(a(0)) {
                        Some(v) if set_fd(a(1), v) => a(1) as isize,
                        Some(_) => EBADF,
                        None => EBADF,
                    }
                }
            }
        }
        25 => 0, // fcntl: accept CLOEXEC and status queries
        29 => {
            // ioctl: nginx uses FIONBIO on listening sockets
            if a(1) == 0x5421 { 0 } else { ENOTTY }
        }
        32 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::File { index, pos: _ }) => {
                    set_fd(a(0), Fd::File { index, pos: 0 });
                    0
                }
                _ => -29,
            }
        },
        33 => unsafe {
            match get_fd(a(0)) {
                Some(v) => {
                    set_fd(a(0), v);
                    0
                }
                None => EBADF,
            }
        },
        34 => 0, // mkdirat: nginx's compiled-in temp directories are virtual
        35 => {
            // nanosleep
            let p = unsafe { user_bytes(a(0), 16) };
            let sec = u64::from_ne_bytes(p[0..8].try_into().unwrap());
            let ns = u64::from_ne_bytes(p[8..16].try_into().unwrap());
            let until = arch::time().wrapping_add(sec.saturating_mul(10_000_000) + ns / 100);
            while arch::time().wrapping_sub(until) as i64 > 0 {}
            0
        }
        48 => unsafe {
            match user_cstr(a(1)) {
                Some(p) if vfs::exists(p) => 0,
                _ => ENOENT,
            }
        },
        53 | 54 => 0, // fchmodat/fchownat on virtual nginx directories
        56 => unsafe {
            match user_cstr(a(1)) {
                Some(p) => open_path(p, a(2)),
                None => ENOENT,
            }
        },
        57 => unsafe {
            if let Some(v) = get_fd(a(0)) {
                if let Fd::Socket { handle } = v {
                    net::close(handle);
                }
                set_fd(a(0), Fd::Empty);
                0
            } else {
                EBADF
            }
        },
        62 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::File { index, .. }) => {
                    if vfs::data(index).is_some() {
                        let p = if a(1) == 0 { 0 } else { a(1) };
                        let _ = set_fd(a(0), Fd::File { index, pos: p });
                        p as isize
                    } else {
                        EBADF
                    }
                }
                _ => EBADF,
            }
        },
        63 => unsafe {
            let out = user_bytes_mut(a(1), a(2));
            match get_fd(a(0)) {
                Some(Fd::Socket { handle }) => net::recv(handle, out),
                _ => read_file(a(0), out),
            }
        },
        67 => unsafe {
            let out = user_bytes_mut(a(1), a(2));
            match get_fd(a(0)) {
                Some(Fd::File { index, .. }) => read_file_at(index, a(3), out),
                _ => EBADF,
            }
        },
        64 => unsafe { write_fd(a(0), user_bytes(a(1), a(2))) },
        65 => {
            // readv
            let mut total = 0isize;
            for i in 0..a(2).min(16) {
                unsafe {
                    let p = user_bytes(a(1) + i * 16, 16);
                    let base = u64::from_ne_bytes(p[0..8].try_into().unwrap()) as usize;
                    let len = u64::from_ne_bytes(p[8..16].try_into().unwrap()) as usize;
                    let n = match get_fd(a(0)) {
                        Some(Fd::Socket { handle }) => net::recv(handle, user_bytes_mut(base, len)),
                        _ => read_file(a(0), user_bytes_mut(base, len)),
                    };
                    if n < 0 {
                        total = n;
                        break;
                    }
                    total += n;
                    if (n as usize) < len {
                        break;
                    }
                }
            }
            total
        }
        66 => {
            // writev
            let mut total = 0isize;
            for i in 0..a(2).min(16) {
                unsafe {
                    let p = user_bytes(a(1) + i * 16, 16);
                    let base = u64::from_ne_bytes(p[0..8].try_into().unwrap()) as usize;
                    let len = u64::from_ne_bytes(p[8..16].try_into().unwrap()) as usize;
                    let n = write_fd(a(0), user_bytes(base, len));
                    if n < 0 {
                        total = n;
                        break;
                    }
                    total += n;
                }
            }
            total
        }
        68 => {
            // pwrite64: /dev/null is used for nginx's disabled pid file
            unsafe {
                match get_fd(a(0)) {
                    Some(Fd::File {
                        index: usize::MAX, ..
                    }) => a(2) as isize,
                    Some(Fd::Stdout) => {
                        console::write_bytes(user_bytes(a(1), a(2)));
                        a(2) as isize
                    }
                    _ => EBADF,
                }
            }
        }
        78 => {
            // readlinkat
            unsafe {
                match user_cstr(a(1)) {
                    Some("/proc/self/exe") => {
                        let b = b"/usr/sbin/nginx";
                        let n = b.len().min(a(3));
                        user_bytes_mut(a(2), n)[..n].copy_from_slice(&b[..n]);
                        n as isize
                    }
                    _ => ENOENT,
                }
            }
        }
        79 => unsafe {
            match user_cstr(a(1)) {
                Some(p)
                    if p == "/var/lib"
                        || p == "/var/lib/nginx"
                        || p.starts_with("/var/lib/nginx/") =>
                {
                    stat_file(a(2), 0, 0o040755, 0x20000)
                }
                Some(p) => match vfs::lookup(p) {
                    Some(idx) => stat_file(
                        a(2),
                        vfs::data(idx).unwrap().len(),
                        0o100644,
                        (idx as u64) + 1,
                    ),
                    None => ENOENT,
                },
                None => ENOENT,
            }
        },
        80 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::File { index, .. }) => stat_file(
                    a(1),
                    vfs::data(index).map_or(0, |d| d.len()),
                    0o100644,
                    (index as u64) + 1,
                ),
                Some(Fd::Socket { handle }) => {
                    stat_file(a(1), 0, 0o140777, 0x10000 + (handle as u64))
                }
                _ => EBADF,
            }
        },
        93 | 94 => {
            tf.sepc = crate::arch::user_halt as *const () as usize - 4;
            0
        }
        98 => 0,
        96 => 1, // set_tid_address
        99 => 0, // set_robust_list
        101 => 0,
        123 => {
            unsafe {
                if a(2) != 0 && a(1) != 0 {
                    user_bytes_mut(a(2), a(1).min(128)).fill(0);
                    user_bytes_mut(a(2), a(1).min(1))[0] = 1;
                }
            }
            0
        } // sched_getaffinity
        113 => unsafe {
            let p = user_bytes_mut(a(1), 16);
            p[..8].copy_from_slice(&0u64.to_ne_bytes());
            p[8..16].copy_from_slice(&(arch::time() * 100).to_ne_bytes());
            0
        },
        134 | 135 | 136 | 139 | 132 | 133 => 0,
        160 => unsafe {
            let p = user_bytes_mut(a(0), 390);
            p.fill(0);
            p[0..4].copy_from_slice(b"Luna");
            p[65..70].copy_from_slice(b"riscv");
            0
        },
        169 => 0,                   // gettimeofday-ish compatibility
        172 | 173 | 178 => 1,       // getpid/getppid/gettid
        174 | 175 | 176 | 177 => 0, // root uid/gid
        258 => 0,                   // riscv_hwprobe: conservative feature set
        198 => {
            let domain = a(0);
            let ty = a(1) & 0xf;
            if domain != 2 || ty != 1 {
                -97
            } else {
                let handle = net::new_socket();
                if handle == usize::MAX {
                    ENOMEM
                } else {
                    alloc_fd(Fd::Socket { handle })
                }
            }
        }
        200 => unsafe {
            let b = user_bytes(a(1), 16);
            let port = u16::from_be_bytes([b[2], b[3]]);
            match get_fd(a(0)) {
                Some(Fd::Socket { handle }) => net::bind(handle, port),
                _ => ENOTSOCK,
            }
        },
        201 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::Socket { handle }) => net::listen(handle, a(1)),
                _ => ENOTSOCK,
            }
        },
        202 | 242 => {
            // accept / accept4
            unsafe {
                match get_fd(a(0)) {
                    Some(Fd::Socket { handle }) => match net::accept(handle) {
                        Some(h) => {
                            if a(1) != 0 {
                                let (ip, port) = net::peer_addr(h).unwrap_or(([10, 0, 2, 2], 0));
                                fill_sockaddr(a(1), a(2), ip, port);
                            }
                            alloc_fd(Fd::Socket { handle: h })
                        }
                        None => EAGAIN,
                    },
                    _ => ENOTSOCK,
                }
            }
        }
        203 => ENOTCONN,
        204 | 205 => {
            // getsockname / getpeername for nginx connection setup
            unsafe {
                match get_fd(a(0)) {
                    Some(Fd::Socket { .. }) => {
                        let out = user_bytes_mut(a(1), 16);
                        out.fill(0);
                        out[0..2].copy_from_slice(&2u16.to_ne_bytes());
                        out[2..4].copy_from_slice(&80u16.to_be_bytes());
                        if nr == 205 {
                            out[4..8].copy_from_slice(&[10, 0, 2, 2]);
                        }
                        if a(2) != 0 {
                            user_bytes_mut(a(2), core::mem::size_of::<u32>())[0..4]
                                .copy_from_slice(&16u32.to_ne_bytes());
                        }
                        0
                    }
                    _ => ENOTSOCK,
                }
            }
        }
        206 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::Socket { handle }) => net::send(handle, user_bytes(a(1), a(2))),
                _ => ENOTSOCK,
            }
        },
        207 => unsafe {
            match get_fd(a(0)) {
                Some(Fd::Socket { handle }) => net::recv(handle, user_bytes_mut(a(1), a(2))),
                _ => ENOTSOCK,
            }
        },
        208 | 209 | 210 => 0,
        214 => unsafe {
            if a(0) != 0 {
                CURRENT_BRK = a(0);
            }
            CURRENT_BRK as isize
        },
        215 => 0,
        216 => 0, // getdents64
        220 => ENOSYS,
        221 => ENOSYS,
        222 => mmap(a(0), a(1), a(2), a(3), a(4) as isize, a(5)),
        226 => 0,
        260 => ECHILD,
        261 => unsafe {
            if a(3) != 0 {
                let p = user_bytes_mut(a(3), 16);
                p[..8].copy_from_slice(&(8u64 * 1024 * 1024).to_ne_bytes());
                p[8..16].copy_from_slice(&u64::MAX.to_ne_bytes());
            }
            0
        },
        278 => unsafe {
            let out = user_bytes_mut(a(0), a(1));
            for (i, x) in out.iter_mut().enumerate() {
                *x = (arch::time() as u8).wrapping_add(i as u8);
            }
            out.len() as isize
        },
        293 => ENOSYS,
        291 => ENOSYS,
        318 => 0, // getrandom variants on some libc builds
        _ => ENOSYS,
    };
    tf.sepc = tf.sepc.wrapping_add(4);
    tf.set_ret(result);
}

fn mmap(addr: usize, len: usize, _prot: usize, flags: usize, fd: isize, offset: usize) -> isize {
    if len == 0 {
        return EINVAL;
    }
    let size = len.div_ceil(4096) * 4096;
    let base = unsafe {
        if flags & 0x10 != 0 {
            addr & !4095
        } else {
            let b = NEXT_MAP;
            NEXT_MAP = NEXT_MAP.saturating_add(size + 4096);
            b
        }
    };
    if base < 0x8040_0000 || base.saturating_add(size) > 0x9f00_0000 {
        return ENOMEM;
    }
    unsafe {
        ptr::write_bytes(base as *mut u8, 0, size);
        if flags & 0x20 == 0 && fd >= 0 {
            if let Some(Fd::File { index, .. }) = get_fd(fd as usize) {
                if let Some(data) = vfs::data(index) {
                    if offset < data.len() {
                        let n = (data.len() - offset).min(len);
                        ptr::copy_nonoverlapping(data.as_ptr().add(offset), base as *mut u8, n);
                    }
                }
            }
        }
        if fd >= 0
            && offset == 0
            && matches!(get_fd(fd as usize), Some(Fd::File { index, .. }) if index == vfs::CACHE)
        {
            // The cache is used only for its ordinary SONAME entries.  Drop
            // the optional glibc-hwcaps extension so the loader does not
            // depend on an extension format from another distro release.
            ptr::write_unaligned((base + 0x20) as *mut u32, 0);
        }
    }
    base as isize
}
