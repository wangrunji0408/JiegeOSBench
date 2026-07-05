//! File-related syscalls.
use super::*;
use crate::fs::{self, FdEntry, FdObj};
use crate::task::current;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

pub const AT_FDCWD: isize = -100;

const O_WRONLY: usize = 1;
const O_RDWR: usize = 2;
const O_CREAT: usize = 0x40;
const O_EXCL: usize = 0x80;
const O_TRUNC: usize = 0x200;
const O_APPEND: usize = 0x400;
const O_NONBLOCK: usize = 0x800;
const O_DIRECTORY: usize = 0x10000;
const O_CLOEXEC: usize = 0x80000;

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFCHR: u32 = 0o020000;
const S_IFSOCK: u32 = 0o140000;

fn resolve_at(dirfd: isize, path: &str) -> Result<alloc::string::String, i32> {
    let t = current();
    if path.starts_with('/') {
        return Ok(fs::normalize("/", path));
    }
    if dirfd == AT_FDCWD {
        return Ok(fs::normalize(&t.cwd, path));
    }
    match t.fds.get(dirfd as usize).map(|e| &e.obj) {
        Some(FdObj::Dir { path: dp }) => Ok(fs::normalize(dp, path)),
        Some(_) => Err(ENOTDIR),
        None => Err(EBADF),
    }
}

pub fn openat(dirfd: isize, path_ptr: usize, flags: usize, _mode: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let full = resolve_at(dirfd, &path)?;

    // devices
    let obj = match full.as_str() {
        "/dev/null" => Some(FdObj::Null),
        "/dev/zero" => Some(FdObj::Null),
        "/dev/stdout" | "/dev/stderr" | "/dev/console" | "/dev/tty" => Some(FdObj::Stdio),
        "/dev/urandom" | "/dev/random" => Some(FdObj::Null), // read handled below? keep simple: getrandom used instead
        _ => None,
    };
    let t = current();
    if let Some(obj) = obj {
        let fd = t.fds.alloc(FdEntry {
            obj,
            cloexec: flags & O_CLOEXEC != 0,
            nonblock: flags & O_NONBLOCK != 0,
        });
        return Ok(fd);
    }

    let is_dir = fs::with_fs(|f| f.is_dir(&full));
    if is_dir {
        let fd = t.fds.alloc(FdEntry {
            obj: FdObj::Dir { path: full },
            cloexec: flags & O_CLOEXEC != 0,
            nonblock: false,
        });
        return Ok(fd);
    }
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }

    let existing = fs::with_fs(|f| f.lookup_file(&full));
    let data = match existing {
        Some(d) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(EEXIST);
            }
            if flags & O_TRUNC != 0 && (flags & (O_WRONLY | O_RDWR)) != 0 {
                d.lock().clear();
            }
            d
        }
        None => {
            if flags & O_CREAT == 0 {
                return Err(ENOENT);
            }
            fs::with_fs(|f| f.create(&full))
        }
    };
    let fd = t.fds.alloc(FdEntry {
        obj: FdObj::File {
            data,
            pos: 0,
            append: flags & O_APPEND != 0,
            path: full,
        },
        cloexec: flags & O_CLOEXEC != 0,
        nonblock: flags & O_NONBLOCK != 0,
    });
    Ok(fd)
}

pub fn close(fd: usize) -> SysResult {
    let t = current();
    match t.fds.close(fd) {
        Some(e) => {
            if let FdObj::Socket(id) = e.obj {
                crate::net::socket_close(id);
            }
            Ok(0)
        }
        None => Err(EBADF),
    }
}

pub fn close_range(first: usize, last: usize, _flags: usize) -> SysResult {
    let t = current();
    for fd in first..=last.min(t.fds.entries.len().saturating_sub(1)) {
        let _ = t.fds.close(fd);
    }
    Ok(0)
}

pub fn read(fd: usize, buf: usize, len: usize) -> SysResult {
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &mut e.obj {
        FdObj::File { data, pos, .. } => {
            let d = data.lock();
            let n = len.min(d.len().saturating_sub(*pos));
            let dst = user_slice_mut(buf, n)?;
            dst.copy_from_slice(&d[*pos..*pos + n]);
            drop(d);
            *pos += n;
            Ok(n)
        }
        FdObj::Null => Ok(0),
        FdObj::Stdio => Ok(0), // no stdin
        FdObj::Socket(id) => {
            let id = *id;
            let nonblock = e.nonblock;
            crate::net::socket_recv(id, buf, len, nonblock)
        }
        FdObj::EventFd { val, semaphore } => {
            let mut v = val.lock();
            if *v == 0 {
                return Err(EAGAIN);
            }
            let out: u64 = if *semaphore { 1 } else { *v };
            *v -= out;
            write_user(buf, out)?;
            Ok(8)
        }
        _ => Err(EBADF),
    }
}

pub fn write(fd: usize, buf: usize, len: usize) -> SysResult {
    let src = user_slice(buf, len)?;
    write_bytes(fd, src)
}

pub fn write_bytes(fd: usize, src: &[u8]) -> SysResult {
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &mut e.obj {
        FdObj::Stdio => {
            for &b in src {
                crate::sbi::console_putchar(b);
            }
            Ok(src.len())
        }
        FdObj::Null => Ok(src.len()),
        FdObj::File {
            data, pos, append, ..
        } => {
            let mut d = data.lock();
            if *append {
                *pos = d.len();
            }
            if *pos + src.len() > d.len() {
                d.resize(*pos + src.len(), 0);
            }
            d[*pos..*pos + src.len()].copy_from_slice(src);
            drop(d);
            *pos += src.len();
            Ok(src.len())
        }
        FdObj::Socket(id) => {
            let id = *id;
            let nonblock = e.nonblock;
            crate::net::socket_send_bytes(id, src, nonblock)
        }
        FdObj::EventFd { val, .. } => {
            if src.len() < 8 {
                return Err(EINVAL);
            }
            let add = u64::from_le_bytes(src[..8].try_into().unwrap());
            *val.lock() += add;
            Ok(8)
        }
        _ => Err(EBADF),
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IoVec {
    base: usize,
    len: usize,
}

pub fn writev(fd: usize, iov: usize, iovcnt: usize) -> SysResult {
    let mut buf: Vec<u8> = Vec::new();
    for i in 0..iovcnt {
        let v: IoVec = read_user(iov + i * 16)?;
        if v.len == 0 {
            continue;
        }
        buf.extend_from_slice(user_slice(v.base, v.len)?);
    }
    write_bytes(fd, &buf)
}

pub fn readv(fd: usize, iov: usize, iovcnt: usize) -> SysResult {
    let mut total = 0;
    for i in 0..iovcnt {
        let v: IoVec = read_user(iov + i * 16)?;
        if v.len == 0 {
            continue;
        }
        match read(fd, v.base, v.len) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if n < v.len {
                    break;
                }
            }
            Err(e) => {
                if total > 0 {
                    return Ok(total);
                }
                return Err(e);
            }
        }
    }
    Ok(total)
}

pub fn pread64(fd: usize, buf: usize, len: usize, off: usize) -> SysResult {
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &e.obj {
        FdObj::File { data, .. } => {
            let d = data.lock();
            let n = len.min(d.len().saturating_sub(off));
            let dst = user_slice_mut(buf, n)?;
            dst.copy_from_slice(&d[off..off + n]);
            Ok(n)
        }
        _ => Err(ESPIPE),
    }
}

pub fn pwrite64(fd: usize, buf: usize, len: usize, off: usize) -> SysResult {
    let src = user_slice(buf, len)?;
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &mut e.obj {
        FdObj::File { data, .. } => {
            let mut d = data.lock();
            if off + len > d.len() {
                d.resize(off + len, 0);
            }
            d[off..off + len].copy_from_slice(src);
            Ok(len)
        }
        _ => Err(ESPIPE),
    }
}

pub fn lseek(fd: usize, off: isize, whence: usize) -> SysResult {
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &mut e.obj {
        FdObj::File { data, pos, .. } => {
            let size = data.lock().len() as isize;
            let newpos = match whence {
                0 => off,
                1 => *pos as isize + off,
                2 => size + off,
                _ => return Err(EINVAL),
            };
            if newpos < 0 {
                return Err(EINVAL);
            }
            *pos = newpos as usize;
            Ok(*pos)
        }
        _ => Err(ESPIPE),
    }
}

// riscv64 struct stat (asm-generic, 128 bytes)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Stat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    _pad1: u64,
    st_size: i64,
    st_blksize: i32,
    _pad2: i32,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    _unused: [u32; 2],
}

fn mkstat(mode: u32, size: i64, ino: u64) -> Stat {
    let now = crate::time::unix_seconds() as i64;
    Stat {
        st_dev: 1,
        st_ino: ino,
        st_mode: mode,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_size: size,
        st_blksize: 4096,
        st_blocks: (size + 511) / 512,
        st_atime: now,
        st_mtime: now,
        st_ctime: now,
        ..Default::default()
    }
}

fn ino_for(path: &str) -> u64 {
    // stable hash so nginx sees consistent inode numbers
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h | 1
}

pub fn stat_path(full: &str) -> Result<Stat, i32> {
    if let Some(d) = fs::with_fs(|f| f.lookup_file(full)) {
        let size = d.lock().len() as i64;
        return Ok(mkstat(S_IFREG | 0o755, size, ino_for(full)));
    }
    if fs::with_fs(|f| f.is_dir(full)) {
        return Ok(mkstat(S_IFDIR | 0o755, 4096, ino_for(full)));
    }
    match full {
        "/dev/null" | "/dev/zero" | "/dev/urandom" | "/dev/random" | "/dev/stdout"
        | "/dev/stderr" => Ok(mkstat(S_IFCHR | 0o666, 0, ino_for(full))),
        _ => Err(ENOENT),
    }
}

pub fn fstatat(dirfd: isize, path_ptr: usize, statbuf: usize, _flags: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let full = resolve_at(dirfd, &path)?;
    let st = stat_path(&full)?;
    write_user(statbuf, st)?;
    Ok(0)
}

pub fn fstat(fd: usize, statbuf: usize) -> SysResult {
    let t = current();
    let e = t.fds.get(fd).ok_or(EBADF)?;
    let st = match &e.obj {
        FdObj::File { data, path, .. } => {
            mkstat(S_IFREG | 0o755, data.lock().len() as i64, ino_for(path))
        }
        FdObj::Dir { path } => mkstat(S_IFDIR | 0o755, 4096, ino_for(path)),
        FdObj::Stdio => mkstat(S_IFCHR | 0o620, 0, 10),
        FdObj::Null => mkstat(S_IFCHR | 0o666, 0, 11),
        FdObj::Socket(_) => mkstat(S_IFSOCK | 0o777, 0, 12),
        _ => mkstat(S_IFREG | 0o644, 0, 13),
    };
    write_user(statbuf, st)?;
    Ok(0)
}

pub fn statx(dirfd: isize, path_ptr: usize, _flags: usize, _mask: usize, buf: usize) -> SysResult {
    // struct statx: fill the basic fields musl/glibc look at
    let path = read_cstr(path_ptr)?;
    let st = if path.is_empty() {
        // AT_EMPTY_PATH on dirfd
        let t = current();
        match t.fds.get(dirfd as usize).map(|e| &e.obj) {
            Some(FdObj::File { data, path, .. }) => {
                mkstat(S_IFREG | 0o755, data.lock().len() as i64, ino_for(path))
            }
            Some(FdObj::Dir { path }) => mkstat(S_IFDIR | 0o755, 4096, ino_for(path)),
            Some(_) => mkstat(S_IFCHR | 0o666, 0, 14),
            None => return Err(EBADF),
        }
    } else {
        let full = resolve_at(dirfd, &path)?;
        stat_path(&full)?
    };
    // statx layout
    check_user_range(buf, 256)?;
    unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 256) };
    write_user(buf, 0x7ffu32)?; // stx_mask
    write_user(buf + 4, 4096u32)?; // blksize
    write_user(buf + 0x1c, 1u32)?; // nlink
    write_user(buf + 0x20, 0u32)?; // uid
    write_user(buf + 0x24, 0u32)?; // gid
    write_user(buf + 0x28, st.st_mode as u16)?; // mode
    write_user(buf + 0x30, ino_for("x"))?; // ino
    write_user(buf + 0x38, st.st_size as u64)?; // size
    write_user(buf + 0x40, st.st_blocks as u64)?; // blocks
    Ok(0)
}

pub fn faccessat(dirfd: usize, path_ptr: usize, _mode: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let full = resolve_at(dirfd as isize, &path)?;
    stat_path(&full)?;
    Ok(0)
}

pub fn getcwd(buf: usize, size: usize) -> SysResult {
    let t = current();
    let cwd = t.cwd.clone();
    let bytes = cwd.as_bytes();
    if size < bytes.len() + 1 {
        return Err(ERANGE);
    }
    let dst = user_slice_mut(buf, bytes.len() + 1)?;
    dst[..bytes.len()].copy_from_slice(bytes);
    dst[bytes.len()] = 0;
    Ok(buf)
}

pub fn chdir(path_ptr: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let t = current();
    let full = fs::normalize(&t.cwd, &path);
    if !fs::with_fs(|f| f.is_dir(&full)) {
        return Err(ENOENT);
    }
    t.cwd = full;
    Ok(0)
}

pub fn mkdirat(dirfd: usize, path_ptr: usize, _mode: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let full = resolve_at(dirfd as isize, &path)?;
    fs::with_fs(|f| f.dirs.insert(full.clone()));
    Ok(0)
}

pub fn unlinkat(dirfd: usize, path_ptr: usize, _flags: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    let full = resolve_at(dirfd as isize, &path)?;
    if fs::with_fs(|f| f.unlink(&full)) {
        Ok(0)
    } else {
        Err(ENOENT)
    }
}

pub fn ftruncate(fd: usize, len: usize) -> SysResult {
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match &mut e.obj {
        FdObj::File { data, .. } => {
            data.lock().resize(len, 0);
            Ok(0)
        }
        _ => Err(EBADF),
    }
}

pub fn readlinkat(_dirfd: usize, path_ptr: usize, buf: usize, size: usize) -> SysResult {
    let path = read_cstr(path_ptr)?;
    if path == "/proc/self/exe" {
        let target = b"/usr/sbin/nginx";
        let n = target.len().min(size);
        user_slice_mut(buf, n)?.copy_from_slice(&target[..n]);
        return Ok(n);
    }
    Err(EINVAL) // not a symlink
}

pub fn dup(fd: usize) -> SysResult {
    let t = current();
    let e = t.fds.get(fd).ok_or(EBADF)?.clone();
    if let FdObj::Socket(id) = e.obj {
        crate::net::socket_dup(id);
    }
    Ok(t.fds.alloc(e))
}

pub fn dup3(old: usize, new: usize, _flags: usize) -> SysResult {
    if old == new {
        return Ok(new);
    }
    let t = current();
    let e = t.fds.get(old).ok_or(EBADF)?.clone();
    if let FdObj::Socket(id) = e.obj {
        crate::net::socket_dup(id);
    }
    if let Some(prev) = t.fds.close(new) {
        if let FdObj::Socket(id) = prev.obj {
            crate::net::socket_close(id);
        }
    }
    t.fds.set(new, e);
    Ok(new)
}

pub fn fcntl(fd: usize, cmd: usize, arg: usize) -> SysResult {
    const F_DUPFD: usize = 0;
    const F_GETFD: usize = 1;
    const F_SETFD: usize = 2;
    const F_GETFL: usize = 3;
    const F_SETFL: usize = 4;
    const F_DUPFD_CLOEXEC: usize = 1030;
    let t = current();
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let mut e = t.fds.get(fd).ok_or(EBADF)?.clone();
            e.cloexec = cmd == F_DUPFD_CLOEXEC;
            if let FdObj::Socket(id) = e.obj {
                crate::net::socket_dup(id);
            }
            // allocate >= arg
            let mut newfd = arg;
            while t.fds.get(newfd).is_some() {
                newfd += 1;
            }
            t.fds.set(newfd, e);
            Ok(newfd)
        }
        F_GETFD => {
            let e = t.fds.get(fd).ok_or(EBADF)?;
            Ok(if e.cloexec { 1 } else { 0 })
        }
        F_SETFD => {
            let e = t.fds.get_mut(fd).ok_or(EBADF)?;
            e.cloexec = arg & 1 != 0;
            Ok(0)
        }
        F_GETFL => {
            let e = t.fds.get(fd).ok_or(EBADF)?;
            Ok(if e.nonblock { O_NONBLOCK } else { 0 } | O_RDWR)
        }
        F_SETFL => {
            let e = t.fds.get_mut(fd).ok_or(EBADF)?;
            e.nonblock = arg & O_NONBLOCK != 0;
            Ok(0)
        }
        _ => Ok(0),
    }
}

pub fn ioctl(fd: usize, req: usize, arg: usize) -> SysResult {
    const FIONBIO: usize = 0x5421;
    const FIONREAD: usize = 0x541B;
    const TIOCGWINSZ: usize = 0x5413;
    let t = current();
    let e = t.fds.get_mut(fd).ok_or(EBADF)?;
    match req {
        FIONBIO => {
            let v: i32 = read_user(arg)?;
            e.nonblock = v != 0;
            Ok(0)
        }
        FIONREAD => {
            let n = match &e.obj {
                FdObj::Socket(id) => crate::net::socket_recv_available(*id),
                FdObj::File { data, pos, .. } => data.lock().len().saturating_sub(*pos),
                _ => 0,
            };
            write_user(arg, n as i32)?;
            Ok(0)
        }
        TIOCGWINSZ => Err(ENOTTY),
        _ => {
            if matches!(e.obj, FdObj::Stdio) {
                Err(ENOTTY)
            } else {
                Ok(0)
            }
        }
    }
}

pub fn getdents64(fd: usize, buf: usize, len: usize) -> SysResult {
    // linux_dirent64 { u64 ino; i64 off; u16 reclen; u8 type; char name[] }
    let t = current();
    let e = t.fds.get(fd).ok_or(EBADF)?;
    let FdObj::Dir { path } = &e.obj else {
        return Err(ENOTDIR);
    };
    let path = path.clone();
    // list children (we don't track per-fd dir positions; return all once
    // then empty — emulate via pos stored in... use a hack: replace obj)
    // For nginx we don't expect getdents at all; return 0 entries.
    let _ = (buf, len, path);
    Ok(0)
}

pub fn pipe2(fds_ptr: usize, _flags: usize) -> SysResult {
    // Minimal blind pipe: both ends are /dev/null-ish. nginx (single process)
    // shouldn't need real pipes.
    let t = current();
    let r = t.fds.alloc(FdEntry {
        obj: FdObj::Null,
        cloexec: false,
        nonblock: false,
    });
    let w = t.fds.alloc(FdEntry {
        obj: FdObj::Null,
        cloexec: false,
        nonblock: false,
    });
    write_user(fds_ptr, r as i32)?;
    write_user(fds_ptr + 4, w as i32)?;
    Ok(0)
}

pub fn eventfd2(initval: usize, flags: usize) -> SysResult {
    let t = current();
    let fd = t.fds.alloc(FdEntry {
        obj: FdObj::EventFd {
            val: alloc::sync::Arc::new(spin::Mutex::new(initval as u64)),
            semaphore: flags & 1 != 0,
        },
        cloexec: flags & 0x80000 != 0,
        nonblock: flags & 0x800 != 0,
    });
    Ok(fd)
}

pub fn sendfile(out_fd: usize, in_fd: usize, off_ptr: usize, count: usize) -> SysResult {
    let t = current();
    let in_e = t.fds.get(in_fd).ok_or(EBADF)?;
    let FdObj::File { data, pos, .. } = &in_e.obj else {
        return Err(EINVAL);
    };
    let start = if off_ptr != 0 {
        read_user::<i64>(off_ptr)? as usize
    } else {
        *pos
    };
    let chunk: Vec<u8> = {
        let d = data.lock();
        let n = count.min(d.len().saturating_sub(start));
        d[start..start + n].to_vec()
    };
    let sent = write_bytes(out_fd, &chunk)?;
    if off_ptr != 0 {
        write_user(off_ptr, (start + sent) as i64)?;
    } else if let Some(e) = current().fds.get_mut(in_fd) {
        if let FdObj::File { pos, .. } = &mut e.obj {
            *pos += sent;
        }
    }
    Ok(sent)
}

pub fn stat_is_reg(full: &str) -> bool {
    fs::with_fs(|f| f.lookup_file(full).is_some())
}

pub fn _debug_list() -> alloc::string::String {
    fs::with_fs(|f| {
        let mut s = alloc::string::String::new();
        for k in f.files.keys() {
            s.push_str(&format!("{}\n", k));
        }
        s
    })
}
