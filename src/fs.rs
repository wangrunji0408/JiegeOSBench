// In-memory filesystem: cpio (read-only) rootfs + RAM overlay, plus the fd table.
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;

pub static ROOTFS: &[u8] = include_bytes!("../rootfs.cpio");

pub const S_IFMT: u32 = 0o170000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_CLOEXEC: u32 = 0o2000000;

#[derive(Clone, Copy)]
pub enum DataRef {
    Static(&'static [u8]),
    Ram(usize),
}

pub struct Node {
    pub path: Vec<u8>,
    pub mode: u32,
    pub data: DataRef,
    pub symlink: Option<Vec<u8>>,
    pub ino: u64,
    pub deleted: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FdKind {
    File,
    DevNull,
    DevZero,
    Console,
    Socket(u32),
    Epoll(u32),
    PipeRead(u32),
    PipeWrite(u32),
}

#[derive(Clone)]
pub struct File {
    pub kind: FdKind,
    pub node: usize,
    pub pos: u64,
    pub flags: u32,
}

static mut NODES: Vec<Node> = Vec::new();
static mut RAMFILES: Vec<Vec<u8>> = Vec::new();
static mut CWD: Vec<u8> = Vec::new();
static mut FILES: Vec<Option<File>> = Vec::new();
static mut NEXT_INO: u64 = 1000;

pub fn nodes() -> &'static mut Vec<Node> {
    unsafe { &mut NODES }
}
pub fn ramfiles() -> &'static mut Vec<Vec<u8>> {
    unsafe { &mut RAMFILES }
}
pub fn files() -> &'static mut Vec<Option<File>> {
    unsafe { &mut FILES }
}
pub fn cwd() -> &'static mut Vec<u8> {
    unsafe { &mut CWD }
}

fn parse_hex(s: &[u8]) -> usize {
    let mut v = 0usize;
    for &c in s {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        };
        v = v * 16 + d as usize;
    }
    v
}

pub fn init() {
    unsafe {
        NEXT_INO = 1000;
        NODES.clear();
        RAMFILES.clear();
        CWD.clear();
        CWD.push(b'/');
        FILES = vec![None; 1024];
    }
    parse_cpio();
    // add synthesised device nodes
    for p in [
        &b"/dev/null"[..],
        &b"/dev/zero"[..],
        &b"/dev/console"[..],
        &b"/dev/tty"[..],
    ] {
        if find(p).is_none() {
            add_node(p.to_vec(), S_IFCHR | 0o666, DataRef::Static(&[]), None);
        }
    }
    // console fds
    for _ in 0..3 {
        let fd = alloc_fd(File {
            kind: FdKind::Console,
            node: 0,
            pos: 0,
            flags: O_RDWR,
        });
        let _ = fd;
    }
}

fn parse_cpio() {
    let data = ROOTFS;
    let mut pos = 0usize;
    while pos + 110 <= data.len() {
        if &data[pos..pos + 6] != b"070701" {
            break;
        }
        let f = |i: usize| parse_hex(&data[pos + 6 + i * 8..pos + 6 + i * 8 + 8]);
        let mode = f(1) as u32;
        let filesize = f(6);
        let namesize = f(11);
        if namesize == 0 || namesize > 4096 {
            break;
        }
        let name_start = pos + 110;
        let name_end = name_start + namesize;
        if name_end > data.len() {
            break;
        }
        let name = &data[name_start..name_end - 1];
        let hdr_end = (name_end + 3) & !3;
        let file_end = hdr_end + filesize;
        if file_end > data.len() {
            break;
        }
        if name == b"TRAILER!!!" {
            break;
        }
        let mut path = Vec::new();
        if name == b"." || name.is_empty() {
            path.push(b'/');
        } else {
            path.push(b'/');
            path.extend_from_slice(name);
        }
        match mode & S_IFMT {
            S_IFDIR => {
                add_node(path, mode, DataRef::Static(&[]), None);
            }
            S_IFLNK => {
                let target = data[hdr_end..file_end].to_vec();
                add_node(path, mode, DataRef::Static(&[]), Some(target));
            }
            _ => {
                add_node(
                    path,
                    mode,
                    DataRef::Static(&data[hdr_end..file_end]),
                    None,
                );
            }
        }
        pos = (file_end + 3) & !3;
    }
    // synthesize parent directories
    let count = unsafe { NODES.len() };
    for i in 0..count {
        let path = unsafe { NODES[i].path.clone() };
        let mut p = Vec::new();
        let comps: Vec<&[u8]> = path.split(|&b| b == b'/').filter(|c| !c.is_empty()).collect();
        for (k, c) in comps.iter().enumerate() {
            p.push(b'/');
            p.extend_from_slice(c);
            if k + 1 != comps.len() && find(&p).is_none() {
                add_node(p.clone(), S_IFDIR | 0o755, DataRef::Static(&[]), None);
            }
        }
    }
}

pub fn add_node(path: Vec<u8>, mode: u32, data: DataRef, symlink: Option<Vec<u8>>) -> usize {
    unsafe {
        NEXT_INO += 1;
        NODES.push(Node {
            path,
            mode,
            data,
            symlink,
            ino: NEXT_INO,
            deleted: false,
        });
        NODES.len() - 1
    }
}

pub fn find(path: &[u8]) -> Option<usize> {
    unsafe {
        for (i, n) in NODES.iter().enumerate() {
            if !n.deleted && n.path == path {
                return Some(i);
            }
        }
    }
    None
}

pub fn get(i: usize) -> &'static Node {
    unsafe { &NODES[i] }
}

pub fn get_mut(i: usize) -> &'static mut Node {
    unsafe { &mut NODES[i] }
}

pub fn norm(path: &[u8]) -> Vec<u8> {
    let mut full = Vec::new();
    if path.first() == Some(&b'/') {
        full.extend_from_slice(path);
    } else {
        let cwd = unsafe { CWD.clone() };
        full.extend_from_slice(&cwd);
        if cwd.last() != Some(&b'/') {
            full.push(b'/');
        }
        full.extend_from_slice(path);
    }
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in full.split(|&b| b == b'/') {
        if c.is_empty() || c == b"." {
            continue;
        }
        if c == b".." {
            parts.pop();
        } else {
            parts.push(c);
        }
    }
    let mut out = Vec::new();
    for p in parts {
        out.push(b'/');
        out.extend_from_slice(p);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

fn parent_of(path: &[u8]) -> Vec<u8> {
    if path == b"/" {
        return b"/".to_vec();
    }
    let mut p = path.to_vec();
    while p.len() > 1 && p.last() != Some(&b'/') {
        p.pop();
    }
    while p.len() > 1 && p.last() == Some(&b'/') {
        p.pop();
    }
    if p.is_empty() {
        p.push(b'/');
    }
    p
}

/// Resolve `path` (which may be relative) following symlinks.
/// Returns the node index of the final component.
pub fn lookup(path: &[u8], follow_last: bool) -> Option<usize> {
    let full = norm(path);
    let mut comps: Vec<Vec<u8>> = full
        .split(|&b| b == b'/')
        .filter(|c| !c.is_empty())
        .map(|c| c.to_vec())
        .collect();
    let mut cur: Vec<u8> = Vec::new();
    let mut hops = 0;
    let mut i = 0;
    let mut last_idx = find(b"/");
    while i < comps.len() {
        let comp = comps[i].clone();
        let mut next = cur.clone();
        next.push(b'/');
        next.extend_from_slice(&comp);
        match find(&next) {
            Some(idx) => {
                let is_link = get(idx).mode & S_IFMT == S_IFLNK;
                if is_link && (i + 1 < comps.len() || follow_last) {
                    hops += 1;
                    if hops > 40 {
                        return None;
                    }
                    let target = get(idx).symlink.clone()?;
                    let mut new_comps: Vec<Vec<u8>> = Vec::new();
                    if target.first() != Some(&b'/') {
                        new_comps = cur
                            .split(|&b| b == b'/')
                            .filter(|c| !c.is_empty())
                            .map(|c| c.to_vec())
                            .collect();
                    }
                    for c in target.split(|&b| b == b'/') {
                        if c.is_empty() || c == b"." {
                            continue;
                        }
                        if c == b".." {
                            new_comps.pop();
                        } else {
                            new_comps.push(c.to_vec());
                        }
                    }
                    new_comps.extend(comps[i + 1..].iter().cloned());
                    comps = new_comps;
                    cur.clear();
                    i = 0;
                    last_idx = find(b"/");
                    continue;
                }
                last_idx = Some(idx);
                cur = next;
                i += 1;
            }
            None => return None,
        }
    }
    last_idx
}

pub fn node_size(idx: usize) -> u64 {
    let n = get(idx);
    match n.data {
        DataRef::Static(d) => d.len() as u64,
        DataRef::Ram(i) => unsafe { RAMFILES[i].len() as u64 },
    }
}

pub fn node_data(idx: usize) -> &'static [u8] {
    let n = get(idx);
    match n.data {
        DataRef::Static(d) => d,
        DataRef::Ram(i) => unsafe { &*(&RAMFILES[i] as *const Vec<u8> as *const [u8]) },
    }
}

pub fn read_file(path: &[u8]) -> Option<&'static [u8]> {
    let idx = lookup(path, true)?;
    if get(idx).mode & S_IFMT != S_IFREG {
        return None;
    }
    Some(node_data(idx))
}

fn ensure_ram(idx: usize) {
    let n = get(idx);
    if let DataRef::Static(d) = n.data {
        let copy = d.to_vec();
        unsafe {
            RAMFILES.push(copy);
            get_mut(idx).data = DataRef::Ram(RAMFILES.len() - 1);
        }
    }
}

pub fn truncate_node(idx: usize) {
    ensure_ram(idx);
    if let DataRef::Ram(i) = get(idx).data {
        unsafe {
            RAMFILES[i].clear();
        }
    }
}

pub fn write_node(idx: usize, pos: u64, buf: &[u8]) -> usize {
    ensure_ram(idx);
    let i = match get(idx).data {
        DataRef::Ram(i) => i,
        _ => return 0,
    };
    unsafe {
        let f = &mut RAMFILES[i];
        let pos = pos as usize;
        if pos > f.len() {
            f.resize(pos, 0);
        }
        let end = pos + buf.len();
        if end > f.len() {
            f.resize(end, 0);
        }
        f[pos..end].copy_from_slice(buf);
    }
    buf.len()
}

fn is_special(path: &[u8]) -> Option<FdKind> {
    match path {
        b"/dev/null" => Some(FdKind::DevNull),
        b"/dev/zero" => Some(FdKind::DevZero),
        b"/dev/console" | b"/dev/tty" => Some(FdKind::Console),
        b"/dev/stdin" | b"/dev/stdout" | b"/dev/stderr" => Some(FdKind::Console),
        _ => None,
    }
}

pub fn alloc_fd(file: File) -> usize {
    unsafe {
        for (i, f) in FILES.iter_mut().enumerate() {
            if f.is_none() {
                *f = Some(file);
                return i;
            }
        }
    }
    usize::MAX
}

pub fn open(path: &[u8], flags: u32, mode: u32) -> Result<usize, i32> {
    let p = norm(path);
    if let Some(kind) = is_special(&p) {
        let fd = alloc_fd(File {
            kind,
            node: 0,
            pos: 0,
            flags,
        });
        if fd == usize::MAX {
            return Err(24);
        }
        return Ok(fd);
    }
    let mut idx = lookup(&p, true);
    if idx.is_none() {
        if flags & O_CREAT == 0 {
            return Err(2); // ENOENT
        }
        let parent = parent_of(&p);
        let pidx = match lookup(&parent, true) {
            Some(i) if get(i).mode & S_IFMT == S_IFDIR => i,
            _ => return Err(2),
        };
        let _ = pidx;
        idx = Some(add_node(
            p.clone(),
            S_IFREG | (mode & 0o7777),
            DataRef::Static(&[]),
            None,
        ));
    }
    let idx = idx.unwrap();
    let n = get(idx);
    let ft = n.mode & S_IFMT;
    if flags & O_DIRECTORY != 0 && ft != S_IFDIR {
        return Err(20); // ENOTDIR
    }
    if ft == S_IFDIR && flags & (O_WRONLY | O_RDWR) != 0 {
        return Err(21); // EISDIR
    }
    if flags & O_TRUNC != 0 && ft == S_IFREG {
        truncate_node(idx);
    }
    let fd = alloc_fd(File {
        kind: if ft == S_IFDIR { FdKind::File } else { FdKind::File },
        node: idx,
        pos: 0,
        flags,
    });
    if fd == usize::MAX {
        return Err(24);
    }
    Ok(fd)
}

pub fn close(fd: usize) -> i32 {
    let f = unsafe { FILES.get(fd).cloned().flatten() };
    match f {
        Some(file) => {
            match file.kind {
                FdKind::Socket(id) => crate::net::close_socket(id),
                FdKind::Epoll(id) => crate::net::epoll_close(id),
                FdKind::PipeRead(id) | FdKind::PipeWrite(id) => crate::net::pipe_close(id),
                _ => {}
            }
            unsafe {
                FILES[fd] = None;
            }
            0
        }
        None => -9, // EBADF
    }
}

pub fn file(fd: usize) -> Option<&'static mut File> {
    unsafe {
        if fd < FILES.len() {
            FILES[fd].as_mut()
        } else {
            None
        }
    }
}

pub fn read_fd(fd: usize, buf: &mut [u8]) -> isize {
    let f = match file(fd) {
        Some(f) => f,
        None => return -9,
    };
    match f.kind {
        FdKind::File => {
            let n = get(f.node);
            if n.mode & S_IFMT == S_IFDIR {
                return -21;
            }
            let size = node_size(f.node);
            if f.pos >= size {
                return 0;
            }
            let avail = (size - f.pos) as usize;
            let n = avail.min(buf.len());
            let data = node_data(f.node);
            buf[..n].copy_from_slice(&data[f.pos as usize..f.pos as usize + n]);
            f.pos += n as u64;
            n as isize
        }
        FdKind::DevZero => {
            for b in buf.iter_mut() {
                *b = 0;
            }
            buf.len() as isize
        }
        FdKind::DevNull => 0,
        FdKind::Console => 0,
        FdKind::Socket(id) => crate::net::recv(id, buf),
        FdKind::PipeRead(id) => crate::net::pipe_read(id, buf),
        _ => -9,
    }
}

pub fn write_fd(fd: usize, buf: &[u8]) -> isize {
    let f = match file(fd) {
        Some(f) => f,
        None => return -9,
    };
    match f.kind {
        FdKind::File => {
            let n = get(f.node);
            if n.mode & S_IFMT == S_IFDIR {
                return -21;
            }
            let mut pos = f.pos;
            if f.flags & O_APPEND != 0 {
                pos = node_size(f.node);
            }
            let written = write_node(f.node, pos, buf);
            f.pos = pos + written as u64;
            written as isize
        }
        FdKind::DevNull => buf.len() as isize,
        FdKind::Console => {
            for &b in buf {
                crate::uart::putc(b);
                if b == b'\n' {
                    crate::uart::putc(b'\r');
                }
            }
            buf.len() as isize
        }
        FdKind::DevZero => buf.len() as isize,
        FdKind::Socket(id) => crate::net::send(id, buf),
        FdKind::PipeWrite(id) => crate::net::pipe_write(id, buf),
        _ => -9,
    }
}

pub fn lseek(fd: usize, off: i64, whence: i32) -> i64 {
    let f = match file(fd) {
        Some(f) => f,
        None => return -9,
    };
    let size = if f.kind == FdKind::File {
        node_size(f.node) as i64
    } else {
        0
    };
    let new = match whence {
        0 => off,
        1 => f.pos as i64 + off,
        2 => size + off,
        _ => return -22,
    };
    if new < 0 {
        return -22;
    }
    f.pos = new as u64;
    new
}

pub fn readdir(buf: &mut [u8], pos: &mut u64) -> isize {
    // unused placeholder
    let _ = (buf, pos);
    -22
}

pub fn getdents(fd: usize, buf: &mut [u8]) -> isize {
    let f = match file(fd) {
        Some(f) => f,
        None => return -9,
    };
    if f.kind != FdKind::File {
        return -9;
    }
    let dir_idx = f.node;
    let dir = get(dir_idx);
    if dir.mode & S_IFMT != S_IFDIR {
        return -20;
    }
    let mut names: Vec<(u64, u8, Vec<u8>)> = Vec::new();
    names.push((dir.ino, 4, b".".to_vec()));
    let parent = parent_of(&dir.path);
    let pino = find(&parent).map(|i| get(i).ino).unwrap_or(dir.ino);
    names.push((pino, 4, b"..".to_vec()));
    let mut seen: Vec<Vec<u8>> = Vec::new();
    unsafe {
        for n in NODES.iter() {
            if n.deleted || n.path == dir.path {
                continue;
            }
            let p = parent_of(&n.path);
            if p == dir.path {
                let base = n
                    .path
                    .iter()
                    .rev()
                    .take_while(|&&b| b != b'/')
                    .cloned()
                    .collect::<Vec<u8>>();
                let base: Vec<u8> = base.into_iter().rev().collect();
                if seen.contains(&base) {
                    continue;
                }
                seen.push(base.clone());
                let ty = match n.mode & S_IFMT {
                    S_IFDIR => 4,
                    S_IFLNK => 10,
                    S_IFCHR => 2,
                    _ => 8,
                };
                names.push((n.ino, ty, base));
            }
        }
    }
    let mut off = *f.pos as usize;
    let mut written = 0usize;
    while off < names.len() {
        let (ino, ty, name) = &names[off];
        let reclen = (19 + name.len() + 1 + 7) & !7;
        if written + reclen > buf.len() {
            break;
        }
        let b = &mut buf[written..written + reclen];
        for x in b.iter_mut() {
            *x = 0;
        }
        b[0..8].copy_from_slice(&ino.to_ne_bytes());
        b[8..16].copy_from_slice(&((off as u64) + 1).to_ne_bytes());
        b[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
        b[18] = *ty;
        b[19..19 + name.len()].copy_from_slice(name);
        written += reclen;
        off += 1;
    }
    *f.pos = off as u64;
    written as isize
}

pub fn readlink(path: &[u8], buf: &mut [u8]) -> isize {
    let idx = match lookup(path, false) {
        Some(i) => i,
        None => return -2,
    };
    let n = get(idx);
    if n.mode & S_IFMT != S_IFLNK {
        return -22;
    }
    let t = match &n.symlink {
        Some(t) => t,
        None => return -22,
    };
    let l = t.len().min(buf.len());
    buf[..l].copy_from_slice(&t[..l]);
    l as isize
}

pub fn fill_stat(idx: usize, out: *mut u8) {
    let n = get(idx);
    let size = node_size(idx);
    unsafe {
        let p = out as *mut u64;
        ptr::write_unaligned(p, 0x100);
        ptr::write_unaligned(p.add(1), n.ino);
        ptr::write_unaligned(p.add(2) as *mut u32, n.mode);
        ptr::write_unaligned(p.add(2) as *mut u32, n.mode);
        ptr::write_unaligned(p.add(2).cast::<u32>().add(1), 1u32); // nlink
        ptr::write_unaligned(p.add(2).cast::<u32>().add(2), 0u32); // uid
        ptr::write_unaligned(p.add(2).cast::<u32>().add(3), 0u32); // gid
        ptr::write_unaligned(p.add(4), 0u64); // rdev
        ptr::write_unaligned(p.add(5), 0u64); // pad
        ptr::write_unaligned(p.add(6) as *mut i64, size as i64);
        ptr::write_unaligned(p.add(7) as *mut i32, 4096i32); // blksize
        ptr::write_unaligned(p.add(7).cast::<i32>().add(1), 0i32);
        ptr::write_unaligned(p.add(8) as *mut i64, ((size + 511) / 512) as i64);
        for i in 0..6 {
            ptr::write_unaligned(p.add(9 + i), 0u64);
        }
        ptr::write_unaligned(p.add(15), 0u64);
    }
}

pub fn stat_path(path: &[u8], follow: bool, out: *mut u8) -> i32 {
    match lookup(path, follow) {
        Some(i) => {
            fill_stat(i, out);
            0
        }
        None => -2,
    }
}

pub fn fstat(fd: usize, out: *mut u8) -> i32 {
    match file(fd) {
        Some(f) => {
            if f.kind == FdKind::File {
                fill_stat(f.node, out);
            } else {
                unsafe {
                    for i in 0..128 {
                        *out.add(i) = 0;
                    }
                    let p = out as *mut u64;
                    ptr::write_unaligned(p.add(1), 5u64);
                    ptr::write_unaligned(p.add(2) as *mut u32, S_IFCHR | 0o666);
                    ptr::write_unaligned(p.add(7) as *mut i32, 4096);
                }
            }
            0
        }
        None => -9,
    }
}

pub fn access(path: &[u8], mode: i32) -> i32 {
    match lookup(path, true) {
        Some(idx) => {
            let n = get(idx);
            if mode & 2 != 0 && n.mode & S_IFMT != S_IFDIR && !matches!(n.data, DataRef::Ram(_)) {
                // read-only cpio file: writes denied
                return -30; // EROFS
            }
            0
        }
        None => -2,
    }
}
