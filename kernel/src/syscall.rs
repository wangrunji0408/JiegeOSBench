use crate::file::*;
use crate::trap::TrapContext;
use crate::{fs, net, task, time};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use spin::Mutex;

const TRACE: bool = true;

// errno
const EPERM: isize = -1;
const ENOENT: isize = -2;
const EBADF: isize = -9;
const EAGAIN: isize = -11;
const ENOMEM: isize = -12;
const EFAULT: isize = -14;
const EEXIST: isize = -17;
const ENOTDIR: isize = -20;
const EISDIR: isize = -21;
const EINVAL: isize = -22;
const ENOSYS: isize = -38;
const ENOTSOCK: isize = -88;
const EOPNOTSUPP: isize = -95;

// ---------- user memory helpers (SUM enabled during syscalls) ----------

unsafe fn uslice<'a>(ptr: usize, len: usize) -> &'a mut [u8] {
    core::slice::from_raw_parts_mut(ptr as *mut u8, len)
}

fn read_cstr(ptr: usize) -> String {
    let mut s = String::new();
    if ptr == 0 {
        return s;
    }
    let mut p = ptr;
    unsafe {
        loop {
            let c = *(p as *const u8);
            if c == 0 {
                break;
            }
            s.push(c as char);
            p += 1;
            if s.len() > 4096 {
                break;
            }
        }
    }
    s
}

unsafe fn write_val<T>(ptr: usize, val: T) {
    (ptr as *mut T).write_unaligned(val);
}

// ---------- open flags & paths ----------

fn resolve_open(path: &str, flags: i32) -> Result<FileKind, isize> {
    match path {
        "/dev/null" => return Ok(FileKind::Null),
        "/dev/zero" => return Ok(FileKind::Zero),
        "/dev/random" | "/dev/urandom" => return Ok(FileKind::Random),
        "/dev/stdout" | "/dev/stderr" | "/dev/tty" | "/dev/console" => {
            return Ok(FileKind::Console)
        }
        _ => {}
    }
    if let Some(node) = fs::lookup(path) {
        let is_dir = node.lock().is_dir;
        if is_dir {
            return Ok(FileKind::Dir(node));
        }
        if flags & O_TRUNC != 0 && (flags & 0x3) != O_RDONLY {
            node.lock().data.clear();
        }
        return Ok(FileKind::File(node));
    }
    if flags & O_CREAT != 0 {
        let node = fs::create_file(path).ok_or(ENOENT)?;
        return Ok(FileKind::File(node));
    }
    Err(ENOENT)
}

fn do_openat(dirfd: isize, path: String, flags: i32) -> isize {
    let full = if path.starts_with('/') {
        path
    } else {
        // Relative paths resolved against "/" (nginx uses absolute prefixes).
        let mut s = String::from("/");
        s.push_str(&path);
        s
    };
    let kind = match resolve_open(&full, flags) {
        Ok(k) => k,
        Err(e) => return e,
    };
    let acc = flags & 0x3;
    let (readable, writable) = match acc {
        O_WRONLY => (false, true),
        O_RDWR => (true, true),
        _ => (true, false),
    };
    let fd = FileDesc {
        kind,
        offset: 0,
        flags,
        readable,
        writable,
    };
    let cloexec = flags & O_CLOEXEC != 0;
    let file = Arc::new(Mutex::new(fd));
    task::current().fds.alloc(file, cloexec) as isize
}

// ---------- read / write ----------

fn do_write(fd: usize, buf: usize, len: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let data = unsafe { uslice(buf, len) };
    let mut f = file.lock();
    if !f.writable && !matches!(f.kind, FileKind::Console) {
        return EBADF;
    }
    match &f.kind {
        FileKind::Console => {
            for &b in data.iter() {
                crate::uart::putchar(b);
            }
            len as isize
        }
        FileKind::Null => len as isize,
        FileKind::File(node) => {
            let mut n = node.lock();
            let off = if f.flags & O_APPEND != 0 {
                n.data.len()
            } else {
                f.offset
            };
            if n.data.len() < off + len {
                n.data.resize(off + len, 0);
            }
            n.data[off..off + len].copy_from_slice(data);
            drop(n);
            f.offset = off + len;
            len as isize
        }
        FileKind::Socket(idx) => {
            let idx = *idx;
            drop(f);
            net::send(idx, data)
        }
        _ => EBADF,
    }
}

fn do_read(fd: usize, buf: usize, len: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let out = unsafe { uslice(buf, len) };
    let mut f = file.lock();
    if !f.readable {
        return EBADF;
    }
    match &f.kind {
        FileKind::Console => 0,
        FileKind::Null => 0,
        FileKind::Zero => {
            out.fill(0);
            len as isize
        }
        FileKind::Random => {
            let mut seed = time::read_time();
            for b in out.iter_mut() {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *b = (seed >> 33) as u8;
            }
            len as isize
        }
        FileKind::File(node) => {
            let n = node.lock();
            let off = f.offset;
            if off >= n.data.len() {
                return 0;
            }
            let take = core::cmp::min(len, n.data.len() - off);
            out[..take].copy_from_slice(&n.data[off..off + take]);
            drop(n);
            f.offset = off + take;
            take as isize
        }
        FileKind::Socket(idx) => {
            let idx = *idx;
            drop(f);
            net::recv(idx, out)
        }
        _ => EBADF,
    }
}

#[repr(C)]
struct IoVec {
    base: usize,
    len: usize,
}

fn do_pread(fd: usize, buf: usize, len: usize, offset: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let out = unsafe { uslice(buf, len) };
    let f = file.lock();
    if let FileKind::File(node) = &f.kind {
        let n = node.lock();
        if offset >= n.data.len() {
            return 0;
        }
        let take = core::cmp::min(len, n.data.len() - offset);
        out[..take].copy_from_slice(&n.data[offset..offset + take]);
        take as isize
    } else {
        // Non-seekable: fall back to a normal read.
        drop(f);
        do_read(fd, buf, len)
    }
}

fn do_pwrite(fd: usize, buf: usize, len: usize, offset: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let data = unsafe { uslice(buf, len) };
    let f = file.lock();
    if let FileKind::File(node) = &f.kind {
        let mut n = node.lock();
        if n.data.len() < offset + len {
            n.data.resize(offset + len, 0);
        }
        n.data[offset..offset + len].copy_from_slice(data);
        len as isize
    } else {
        drop(f);
        do_write(fd, buf, len)
    }
}

fn do_writev(fd: usize, iov: usize, cnt: usize) -> isize {
    let mut total = 0isize;
    for i in 0..cnt {
        let v = unsafe { &*((iov + i * 16) as *const IoVec) };
        if v.len == 0 {
            continue;
        }
        let r = do_write(fd, v.base, v.len);
        if r < 0 {
            return if total > 0 { total } else { r };
        }
        total += r;
        if (r as usize) < v.len {
            break;
        }
    }
    total
}

fn do_readv(fd: usize, iov: usize, cnt: usize) -> isize {
    let mut total = 0isize;
    for i in 0..cnt {
        let v = unsafe { &*((iov + i * 16) as *const IoVec) };
        if v.len == 0 {
            continue;
        }
        let r = do_read(fd, v.base, v.len);
        if r < 0 {
            return if total > 0 { total } else { r };
        }
        total += r;
        if (r as usize) < v.len {
            break;
        }
    }
    total
}

// ---------- stat ----------

fn fill_stat(ptr: usize, size: usize, is_dir: bool, is_chr: bool) {
    unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, 128);
        let mode: u32 = if is_dir {
            0o040000 | 0o755
        } else if is_chr {
            0o020000 | 0o666
        } else {
            0o100000 | 0o644
        };
        write_val::<u64>(ptr + 0, 1); // st_dev
        write_val::<u64>(ptr + 8, 1); // st_ino
        write_val::<u32>(ptr + 16, mode);
        write_val::<u32>(ptr + 20, 1); // nlink
        write_val::<u32>(ptr + 24, 0); // uid
        write_val::<u32>(ptr + 28, 0); // gid
        write_val::<u64>(ptr + 48, size as u64); // st_size
        write_val::<u32>(ptr + 56, 4096); // blksize
        write_val::<u64>(ptr + 64, ((size + 511) / 512) as u64); // blocks
    }
}

fn stat_kind(kind: &FileKind) -> (usize, bool, bool) {
    match kind {
        FileKind::File(n) => (n.lock().data.len(), false, false),
        FileKind::Dir(_) => (0, true, false),
        FileKind::Console | FileKind::Null | FileKind::Zero | FileKind::Random => (0, false, true),
        _ => (0, false, true),
    }
}

fn do_fstat(fd: usize, statbuf: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let f = file.lock();
    let (size, is_dir, is_chr) = stat_kind(&f.kind);
    fill_stat(statbuf, size, is_dir, is_chr);
    0
}

fn do_fstatat(dirfd: isize, path: usize, statbuf: usize, _flags: i32) -> isize {
    let p = read_cstr(path);
    let full = if p.starts_with('/') {
        p
    } else if p.is_empty() {
        // AT_EMPTY_PATH => stat dirfd itself.
        return do_fstat(dirfd as usize, statbuf);
    } else {
        let mut s = String::from("/");
        s.push_str(&p);
        s
    };
    match full.as_str() {
        "/dev/null" | "/dev/zero" | "/dev/random" | "/dev/urandom" | "/dev/stdout"
        | "/dev/stderr" | "/dev/tty" | "/dev/console" => {
            fill_stat(statbuf, 0, false, true);
            return 0;
        }
        _ => {}
    }
    match fs::lookup(&full) {
        Some(node) => {
            let n = node.lock();
            let (is_dir, size) = (n.is_dir, n.data.len());
            drop(n);
            fill_stat(statbuf, size, is_dir, false);
            0
        }
        None => ENOENT,
    }
}

// ---------- mmap / brk ----------

fn do_brk(new: usize) -> isize {
    let t = task::current();
    if new == 0 {
        return t.brk as isize;
    }
    if new > t.brk {
        t.map_user(t.brk, new - t.brk, crate::page_table::PTE_R | crate::page_table::PTE_W | crate::page_table::PTE_X);
    }
    t.brk = new;
    t.brk as isize
}

const MAP_FIXED: i32 = 0x10;
const MAP_ANONYMOUS: i32 = 0x20;

fn do_mmap(addr: usize, length: usize, _prot: i32, flags: i32, fd: isize, offset: usize) -> isize {
    let len = (length + 4095) & !4095;
    let t = task::current();
    let perm = crate::page_table::PTE_R | crate::page_table::PTE_W | crate::page_table::PTE_X;
    let va = if flags & MAP_FIXED != 0 && addr != 0 {
        addr
    } else if addr != 0 && flags & MAP_ANONYMOUS != 0 && false {
        addr
    } else {
        let v = t.mmap_top;
        t.mmap_top += len;
        v
    };
    t.map_user(va, len, perm);
    if flags & MAP_ANONYMOUS == 0 && fd >= 0 {
        // File-backed: copy file contents into the region.
        if let Some(file) = t.fds.get(fd as usize) {
            let f = file.lock();
            if let FileKind::File(node) = &f.kind {
                let n = node.lock();
                let end = core::cmp::min(offset + len, n.data.len());
                if offset < end {
                    unsafe {
                        let dst = uslice(va, end - offset);
                        dst.copy_from_slice(&n.data[offset..end]);
                    }
                }
            }
        }
    }
    va as isize
}

fn do_munmap(addr: usize, length: usize) -> isize {
    let len = (length + 4095) & !4095;
    let t = task::current();
    let mut a = addr & !4095;
    let end = addr + len;
    while a < end {
        if let Some(pa) = t.pt.unmap(a) {
            crate::frame::free(pa);
        }
        a += 4096;
    }
    unsafe {
        core::arch::asm!("sfence.vma");
    }
    0
}

// ---------- socket ----------

const AF_INET: usize = 2;

fn sockaddr_port(addr: usize) -> u16 {
    unsafe {
        let hi = *((addr + 2) as *const u8) as u16;
        let lo = *((addr + 3) as *const u8) as u16;
        (hi << 8) | lo
    }
}

fn write_sockaddr_in(addr: usize, addrlen: usize, ip: u32, port: u16) {
    if addr == 0 {
        return;
    }
    unsafe {
        if addrlen != 0 {
            let cap = *(addrlen as *const u32) as usize;
            if cap >= 16 {
                write_val::<u16>(addr, AF_INET as u16);
                write_val::<u8>(addr + 2, (port >> 8) as u8);
                write_val::<u8>(addr + 3, (port & 0xff) as u8);
                write_val::<u32>(addr + 4, ip.to_be());
            }
            write_val::<u32>(addrlen, 16);
        }
    }
}

fn wrap_socket(idx: usize, nonblock: bool, cloexec: bool) -> isize {
    let fd = FileDesc {
        kind: FileKind::Socket(idx),
        offset: 0,
        flags: if nonblock { O_NONBLOCK } else { 0 },
        readable: true,
        writable: true,
    };
    task::current()
        .fds
        .alloc(Arc::new(Mutex::new(fd)), cloexec) as isize
}

fn sock_idx(fd: usize) -> Option<usize> {
    let file = task::current().fds.get(fd)?;
    let f = file.lock();
    if let FileKind::Socket(i) = f.kind {
        Some(i)
    } else {
        None
    }
}

const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2000000;

// ---------- poll ----------

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 0x001;
const POLLOUT: i16 = 0x004;

fn do_ppoll(fds: usize, nfds: usize, tmo: usize) -> isize {
    let deadline = if tmo == 0 {
        None
    } else {
        let sec = unsafe { *(tmo as *const u64) };
        let nsec = unsafe { *((tmo + 8) as *const u64) };
        Some(time::now_ms() + sec * 1000 + nsec / 1_000_000)
    };
    loop {
        net::poll();
        let mut ready = 0;
        for i in 0..nfds {
            let pf = unsafe { &mut *((fds + i * 8) as *mut PollFd) };
            pf.revents = 0;
            if pf.fd < 0 {
                continue;
            }
            let fd = pf.fd as usize;
            let file = match task::current().fds.get(fd) {
                Some(f) => f,
                None => continue,
            };
            let f = file.lock();
            let (r, w) = match f.kind {
                FileKind::Socket(idx) => (net::readable(idx), net::writable(idx)),
                FileKind::Console => (false, true),
                _ => (true, true),
            };
            if pf.events & POLLIN != 0 && r {
                pf.revents |= POLLIN;
            }
            if pf.events & POLLOUT != 0 && w {
                pf.revents |= POLLOUT;
            }
            if pf.revents != 0 {
                ready += 1;
            }
        }
        if ready > 0 {
            return ready;
        }
        if let Some(d) = deadline {
            if time::now_ms() >= d {
                return 0;
            }
        }
    }
}

// ---------- epoll ----------

struct EpollInst {
    interest: alloc::vec::Vec<(u32, u32, u64)>, // (fd, events, data)
}

static mut EPOLLS: Option<alloc::vec::Vec<Option<EpollInst>>> = None;

fn epolls() -> &'static mut alloc::vec::Vec<Option<EpollInst>> {
    unsafe {
        let e = &mut *core::ptr::addr_of_mut!(EPOLLS);
        if e.is_none() {
            *e = Some(alloc::vec::Vec::new());
        }
        e.as_mut().unwrap()
    }
}

fn do_eventfd(_init: usize) -> isize {
    let fd = FileDesc {
        kind: FileKind::Eventfd(0),
        offset: 0,
        flags: 0,
        readable: true,
        writable: true,
    };
    task::current().fds.alloc(Arc::new(Mutex::new(fd)), false) as isize
}

fn do_epoll_create() -> isize {
    let table = epolls();
    let mut idx = table.len();
    for (i, s) in table.iter().enumerate() {
        if s.is_none() {
            idx = i;
            break;
        }
    }
    let inst = EpollInst {
        interest: alloc::vec::Vec::new(),
    };
    if idx == table.len() {
        table.push(Some(inst));
    } else {
        table[idx] = Some(inst);
    }
    let fd = FileDesc {
        kind: FileKind::Epoll(idx),
        offset: 0,
        flags: 0,
        readable: true,
        writable: true,
    };
    task::current().fds.alloc(Arc::new(Mutex::new(fd)), false) as isize
}

fn epoll_idx(epfd: usize) -> Option<usize> {
    let file = task::current().fds.get(epfd)?;
    let f = file.lock();
    if let FileKind::Epoll(i) = f.kind {
        Some(i)
    } else {
        None
    }
}

fn do_epoll_ctl(epfd: usize, op: usize, fd: usize, ev: usize) -> isize {
    let idx = match epoll_idx(epfd) {
        Some(i) => i,
        None => return EBADF,
    };
    let (events, data) = if ev != 0 {
        unsafe {
            let events = *(ev as *const u32);
            let data = *((ev + 8) as *const u64);
            (events, data)
        }
    } else {
        (0, 0)
    };
    let inst = match epolls().get_mut(idx).and_then(|e| e.as_mut()) {
        Some(i) => i,
        None => return EBADF,
    };
    match op {
        1 => {
            // EPOLL_CTL_ADD
            inst.interest.retain(|e| e.0 != fd as u32);
            inst.interest.push((fd as u32, events, data));
        }
        2 => {
            // EPOLL_CTL_DEL
            inst.interest.retain(|e| e.0 != fd as u32);
        }
        3 => {
            // EPOLL_CTL_MOD
            for e in inst.interest.iter_mut() {
                if e.0 == fd as u32 {
                    e.1 = events;
                    e.2 = data;
                }
            }
        }
        _ => return EINVAL,
    }
    0
}

const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLRDHUP: u32 = 0x2000;

fn fd_ready(fd: usize, want: u32) -> u32 {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return 0,
    };
    let f = file.lock();
    let (r, w) = match f.kind {
        FileKind::Socket(idx) => (net::readable(idx), net::writable(idx)),
        FileKind::Console => (false, true),
        FileKind::Epoll(_) | FileKind::Eventfd(_) => (false, false),
        _ => (true, true),
    };
    let mut revents = 0;
    if r && want & EPOLLIN != 0 {
        revents |= EPOLLIN;
    }
    if w && want & EPOLLOUT != 0 {
        revents |= EPOLLOUT;
    }
    revents
}

fn do_epoll_pwait(epfd: usize, events: usize, maxevents: usize, timeout: isize) -> isize {
    let idx = match epoll_idx(epfd) {
        Some(i) => i,
        None => return EBADF,
    };
    let deadline = if timeout < 0 {
        None
    } else {
        Some(time::now_ms() + timeout as u64)
    };
    loop {
        net::poll();
        let mut count = 0usize;
        // Snapshot interest to avoid borrow issues.
        let snapshot: alloc::vec::Vec<(u32, u32, u64)> =
            match epolls().get(idx).and_then(|e| e.as_ref()) {
                Some(i) => i.interest.clone(),
                None => return EBADF,
            };
        for (fd, want, data) in snapshot {
            if count >= maxevents {
                break;
            }
            let mut revents = fd_ready(fd as usize, want);
            // Report hangup so nginx notices closed connections.
            if let Some(file) = task::current().fds.get(fd as usize) {
                if let FileKind::Socket(sidx) = file.lock().kind {
                    if net::readable(sidx) && !net::writable(sidx) {
                        // may indicate closed; nginx will read and see EOF
                        revents |= EPOLLIN & (want | EPOLLIN);
                    }
                }
            }
            if revents != 0 {
                unsafe {
                    write_val::<u32>(events + count * 16, revents);
                    write_val::<u64>(events + count * 16 + 8, data);
                }
                count += 1;
            }
        }
        if count > 0 {
            return count as isize;
        }
        if timeout == 0 {
            return 0;
        }
        if let Some(d) = deadline {
            if time::now_ms() >= d {
                return 0;
            }
        }
    }
}

// ---------- dispatch ----------

pub fn dispatch(cx: &mut TrapContext) {
    let a0 = cx.a(0);
    let a1 = cx.a(1);
    let a2 = cx.a(2);
    let a3 = cx.a(3);
    let a4 = cx.a(4);
    let a5 = cx.a(5);
    let no = cx.syscall_no();
    if TRACE {
        crate::println!(
            "[sc] no={} a0={:#x} a1={:#x} a2={:#x} a3={:#x}",
            no, a0, a1, a2, a3
        );
    }
    let ret: isize = match no {
        17 => {
            // getcwd
            let s = b"/nginx\0";
            if a1 >= s.len() {
                unsafe { uslice(a0, s.len()).copy_from_slice(s) };
                a0 as isize
            } else {
                ERANGE
            }
        }
        23 => {
            // dup
            match task::current().fds.get(a0) {
                Some(f) => task::current().fds.alloc(f, false) as isize,
                None => EBADF,
            }
        }
        24 => {
            // dup3(oldfd, newfd, flags)
            match task::current().fds.get(a0) {
                Some(f) => {
                    task::current().fds.set(a1, f, a2 as i32 & O_CLOEXEC != 0);
                    a1 as isize
                }
                None => EBADF,
            }
        }
        25 => do_fcntl(a0, a1, a2),
        29 => do_ioctl(a0, a1, a2),
        34 => {
            // mkdirat
            let p = read_cstr(a1);
            let full = abspath(&p);
            fs::mkdir_p(&full);
            0
        }
        48 => 0, // faccessat
        49 => 0, // chdir
        56 => {
            // openat(dirfd, path, flags, mode)
            let p = read_cstr(a1);
            do_openat(a0 as isize, p, a2 as i32)
        }
        57 => {
            // close
            if let Some(idx) = sock_idx(a0) {
                net::close(idx);
            }
            if task::current().fds.close(a0) {
                0
            } else {
                EBADF
            }
        }
        61 => 0, // getdents64: report empty
        62 => do_lseek(a0, a1 as isize, a2),
        63 => do_read(a0, a1, a2),
        64 => do_write(a0, a1, a2),
        65 => do_readv(a0, a1, a2),
        66 => do_writev(a0, a1, a2),
        67 => do_pread(a0, a1, a2, a3),
        68 => do_pwrite(a0, a1, a2, a3),
        73 => do_ppoll(a0, a1, a2),
        20 => do_epoll_create(),
        21 => do_epoll_ctl(a0, a1, a2, a3),
        22 => do_epoll_pwait(a0, a1, a2, a3 as isize),
        19 => do_eventfd(a0),
        78 => ENOSYS, // readlinkat
        79 => do_fstatat(a0 as isize, a1, a2, a3 as i32),
        80 => do_fstat(a0, a1),
        93 | 94 => {
            // exit / exit_group
            crate::println!("[kernel] user exit code {}", a0 as i32);
            crate::sbi::shutdown();
        }
        96 => {
            // set_tid_address
            task::current().tid_address = a0;
            1
        }
        98 => EAGAIN, // futex: no contention in single thread
        99 => 0,      // set_robust_list
        113 => do_clock_gettime(a0, a1),
        115 => do_clock_nanosleep(a2, a3),
        101 => do_nanosleep(a0, a1),
        124 => 0,  // sched_yield
        122 => 0,  // sched_setaffinity
        123 => {
            // sched_getaffinity: set 1 cpu
            if a2 != 0 {
                unsafe { write_val::<u64>(a2, 1) };
            }
            core::cmp::min(a1, 8) as isize
        }
        129 => 0,  // kill
        130 => 0,  // tkill
        131 => 0,  // tgkill
        132 => 0,  // sigaltstack
        133 => 0,  // rt_sigsuspend
        134 => 0,  // rt_sigaction
        135 => 0,  // rt_sigprocmask
        160 => do_uname(a0),
        163 => 0,  // getrlimit (old)
        165 => 0,  // getrusage
        169 => do_gettimeofday(a0),
        172 => 1,  // getpid
        173 => 1,  // getppid
        174 => 1000, // getuid (non-root: skip privilege dropping)
        175 => 1000, // geteuid
        176 => 1000, // getgid
        177 => 1000, // getegid
        178 => 1,  // gettid
        179 => 0,  // sysinfo
        198 => do_socket(a0, a1, a2),
        200 => match sock_idx(a0) {
            Some(idx) => net::bind(idx, sockaddr_port(a1)),
            None => ENOTSOCK,
        },
        201 => match sock_idx(a0) {
            Some(idx) => net::listen(idx, a1),
            None => ENOTSOCK,
        },
        202 => do_accept(a0, a1, a2, 0),
        203 => ECONNREFUSED, // connect: no outbound support
        204 => do_getsockname(a0, a1, a2),
        205 => do_getsockname(a0, a1, a2),
        206 => do_write(a0, a1, a2), // sendto (ignore addr)
        207 => do_read(a0, a1, a2),  // recvfrom (ignore addr)
        208 => 0,                    // setsockopt
        209 => do_getsockopt(a0, a1, a2, a3, a4),
        210 => 0, // shutdown
        211 => do_sendmsg(a0, a1),
        212 => ENOSYS, // recvmsg
        214 => do_brk(a0),
        215 => do_munmap(a0, a1),
        220 => ENOSYS, // clone
        221 => ENOSYS, // execve
        222 => do_mmap(a0, a1, a2 as i32, a3 as i32, a4 as isize, a5),
        226 => 0,      // mprotect (all pages already RWX)
        233 => 0,      // madvise
        242 => do_accept(a0, a1, a2, a3),
        260 => 0,      // wait4
        261 => 0,      // prlimit64
        278 => do_getrandom(a0, a1),
        291 => do_statx(a0, a1, a2, a3, a4),
        _ => {
            crate::println!("[kernel] UNIMPLEMENTED syscall {} (a0={:#x})", no, a0);
            ENOSYS
        }
    };
    cx.set_ret(ret as usize);
}

const ERANGE: isize = -34;
const ECONNREFUSED: isize = -111;

fn abspath(p: &str) -> String {
    if p.starts_with('/') {
        String::from(p)
    } else {
        let mut s = String::from("/");
        s.push_str(p);
        s
    }
}

fn do_socket(domain: usize, ty: usize, _proto: usize) -> isize {
    if domain != AF_INET {
        return -97; // EAFNOSUPPORT
    }
    let nonblock = ty & SOCK_NONBLOCK != 0;
    let cloexec = ty & SOCK_CLOEXEC != 0;
    let idx = net::socket(nonblock);
    wrap_socket(idx, nonblock, cloexec)
}

fn do_accept(fd: usize, addr: usize, addrlen: usize, flags: usize) -> isize {
    let idx = match sock_idx(fd) {
        Some(i) => i,
        None => return ENOTSOCK,
    };
    let listener_nb = net::is_nonblock(idx);
    let child_nb = flags & SOCK_NONBLOCK != 0;
    let cloexec = flags & SOCK_CLOEXEC != 0;
    match net::accept(idx, listener_nb) {
        Ok(new_idx) => {
            net::set_nonblock(new_idx, child_nb);
            if addr != 0 {
                let (ip, port) = net::local_ip_port(new_idx);
                write_sockaddr_in(addr, addrlen, ip, port);
            }
            wrap_socket(new_idx, child_nb, cloexec)
        }
        Err(e) => e,
    }
}

fn do_getsockname(fd: usize, addr: usize, addrlen: usize) -> isize {
    let idx = match sock_idx(fd) {
        Some(i) => i,
        None => return ENOTSOCK,
    };
    let (ip, port) = net::local_ip_port(idx);
    write_sockaddr_in(addr, addrlen, ip, port);
    0
}

fn do_getsockopt(_fd: usize, _level: usize, optname: usize, optval: usize, optlen: usize) -> isize {
    // Return 0 for everything (notably SO_ERROR = 4).
    if optval != 0 && optlen != 0 {
        unsafe {
            let cap = *(optlen as *const u32);
            if cap >= 4 {
                write_val::<u32>(optval, 0);
            }
            write_val::<u32>(optlen, 4);
        }
    }
    let _ = optname;
    0
}

fn do_sendmsg(fd: usize, msg: usize) -> isize {
    // struct msghdr { name, namelen, iov, iovlen, control, ... }
    let iov = unsafe { *((msg + 16) as *const usize) };
    let iovlen = unsafe { *((msg + 24) as *const usize) };
    do_writev(fd, iov, iovlen)
}

fn do_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    match cmd {
        0 | 1030 => {
            // F_DUPFD / F_DUPFD_CLOEXEC
            task::current().fds.alloc_from(arg, file, cmd == 1030) as isize
        }
        1 => file.lock().flags as isize, // F_GETFD -> we track via table; return 0
        2 => 0,                          // F_SETFD
        3 => file.lock().flags as isize, // F_GETFL
        4 => {
            // F_SETFL
            let mut f = file.lock();
            f.flags = arg as i32;
            if let FileKind::Socket(idx) = f.kind {
                net::set_nonblock(idx, arg as i32 & O_NONBLOCK != 0);
            }
            0
        }
        _ => 0,
    }
}

fn do_ioctl(fd: usize, req: usize, arg: usize) -> isize {
    match req {
        0x5421 => {
            // FIONBIO
            let on = unsafe { *(arg as *const i32) } != 0;
            if let Some(idx) = sock_idx(fd) {
                net::set_nonblock(idx, on);
            }
            0
        }
        0x541b => {
            // FIONREAD
            unsafe { write_val::<i32>(arg, 0) };
            0
        }
        _ => 0,
    }
}

fn do_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    let file = match task::current().fds.get(fd) {
        Some(f) => f,
        None => return EBADF,
    };
    let mut f = file.lock();
    let size = match &f.kind {
        FileKind::File(n) => n.lock().data.len(),
        _ => 0,
    };
    let base = match whence {
        0 => 0,             // SEEK_SET
        1 => f.offset as isize, // SEEK_CUR
        2 => size as isize, // SEEK_END
        _ => return EINVAL,
    };
    let newoff = base + offset;
    if newoff < 0 {
        return EINVAL;
    }
    f.offset = newoff as usize;
    newoff
}

fn do_clock_gettime(_clk: usize, tp: usize) -> isize {
    let (sec, nsec) = time::now();
    unsafe {
        write_val::<u64>(tp, sec);
        write_val::<u64>(tp + 8, nsec);
    }
    0
}

fn do_gettimeofday(tv: usize) -> isize {
    let (sec, nsec) = time::now();
    unsafe {
        write_val::<u64>(tv, sec);
        write_val::<u64>(tv + 8, nsec / 1000);
    }
    0
}

fn do_nanosleep(req: usize, _rem: usize) -> isize {
    let sec = unsafe { *(req as *const u64) };
    let nsec = unsafe { *((req + 8) as *const u64) };
    let deadline = time::now_ms() + sec * 1000 + nsec / 1_000_000;
    while time::now_ms() < deadline {
        net::poll();
    }
    0
}

fn do_clock_nanosleep(req: usize, rem: usize) -> isize {
    do_nanosleep(req, rem)
}

fn do_uname(buf: usize) -> isize {
    let fields = [
        "Linux",          // sysname
        "ijiege",         // nodename
        "6.1.0",          // release
        "#1 SMP",         // version
        "riscv64",        // machine
        "",               // domainname
    ];
    unsafe {
        core::ptr::write_bytes(buf as *mut u8, 0, 6 * 65);
        for (i, s) in fields.iter().enumerate() {
            let dst = buf + i * 65;
            uslice(dst, s.len()).copy_from_slice(s.as_bytes());
        }
    }
    0
}

fn do_getrandom(buf: usize, len: usize) -> isize {
    let mut seed = time::read_time();
    let out = unsafe { uslice(buf, len) };
    for b in out.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (seed >> 33) as u8;
    }
    len as isize
}

fn do_statx(dirfd: usize, path: usize, _flags: usize, _mask: usize, statxbuf: usize) -> isize {
    let p = read_cstr(path);
    let full = abspath(&p);
    let (size, is_dir) = match fs::lookup(&full) {
        Some(n) => {
            let node = n.lock();
            (node.data.len(), node.is_dir)
        }
        None => match full.as_str() {
            "/dev/null" | "/dev/zero" | "/dev/urandom" | "/dev/random" => (0, false),
            _ => return ENOENT,
        },
    };
    let _ = dirfd;
    unsafe {
        core::ptr::write_bytes(statxbuf as *mut u8, 0, 256);
        // struct statx: mask(0,u32), blksize(4,u32), attributes(8,u64),
        // nlink(16,u32), uid(20), gid(24), mode(28,u16), ino(32,u64), size(40,u64)
        write_val::<u32>(statxbuf + 0, 0x7ff);
        write_val::<u32>(statxbuf + 4, 4096);
        write_val::<u32>(statxbuf + 16, 1);
        let mode: u16 = if is_dir { 0o040755 } else { 0o100644 };
        write_val::<u16>(statxbuf + 28, mode);
        write_val::<u64>(statxbuf + 32, 1);
        write_val::<u64>(statxbuf + 40, size as u64);
    }
    0
}
