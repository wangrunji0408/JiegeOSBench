//! File-related system calls.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::abi::*;
use crate::fs::epoll::Epoll;
use crate::fs::eventfd::EventFd;
use crate::fs::file::{File, FileOps};
use crate::fs::pipe::create_pipe;
use crate::fs::vfs::{self, Dentry, NodeKind};
use crate::mm::uaccess::*;
use crate::task::current;
use crate::time::monotonic_ns;

pub fn get_file(fd: i32) -> Result<Arc<File>, i32> {
    current().fds().lock().get(fd)
}

/// Resolve the base directory for *at syscalls.
pub fn dir_base(dirfd: i32) -> Result<Arc<Dentry>, i32> {
    if dirfd == AT_FDCWD {
        Ok(current().cwd())
    } else {
        let f = get_file(dirfd)?;
        let d = f.dentry().ok_or(ENOTDIR)?;
        if !d.is_dir() {
            return Err(ENOTDIR);
        }
        Ok(d)
    }
}

pub fn read_path(ptr: usize) -> Result<String, i32> {
    read_string(ptr, 4096)
}

/// Lookup helper for `*at` calls honouring AT_EMPTY_PATH.
fn lookup_at(dirfd: i32, path: &str, follow: bool, flags: u32) -> Result<Arc<Dentry>, i32> {
    if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        if dirfd == AT_FDCWD {
            return Ok(current().cwd());
        }
        return get_file(dirfd)?.dentry().ok_or(ENOENT);
    }
    let base = dir_base(dirfd)?;
    vfs::lookup(&base, path, follow)
}

pub fn install_fd(file: Arc<File>, cloexec: bool) -> SysResult {
    let fd = current().fds().lock().alloc(file, cloexec, 0)?;
    Ok(fd as usize)
}

pub fn sys_openat(dirfd: i32, path: usize, flags: u32, mode: u32) -> SysResult {
    let path = read_path(path)?;
    let base = dir_base(dirfd)?;
    let file = crate::fs::open(&base, &path, flags, mode)?;
    if flags & O_DIRECTORY != 0 && file.dentry().map(|d| !d.is_dir()).unwrap_or(true) {
        return Err(ENOTDIR);
    }
    install_fd(file, flags & O_CLOEXEC != 0)
}

pub fn sys_close(fd: i32) -> SysResult {
    let file = current().fds().lock().close(fd)?;
    drop(file);
    Ok(0)
}

pub fn sys_close_range(first: u32, last: u32, _flags: u32) -> SysResult {
    let fds = current().fds();
    let mut t = fds.lock();
    let mut closed = Vec::new();
    for fd in first..=last.min(4095) {
        if let Ok(f) = t.close(fd as i32) {
            closed.push(f);
        }
    }
    drop(t);
    drop(closed);
    Ok(0)
}

pub fn sys_read(fd: i32, buf: usize, len: usize) -> SysResult {
    let file = get_file(fd)?;
    if len == 0 {
        return Ok(0);
    }
    // Read into a kernel buffer first (may block), then copy out.
    let chunk = len.min(256 * 1024);
    let mut kbuf = alloc::vec![0u8; chunk];
    let n = file.read(&mut kbuf)?;
    copy_to_user(buf, &kbuf[..n])?;
    Ok(n)
}

pub fn sys_write(fd: i32, buf: usize, len: usize) -> SysResult {
    let file = get_file(fd)?;
    if len == 0 {
        return file.write(&[]);
    }
    let chunk = len.min(256 * 1024);
    let kbuf = read_bytes(buf, chunk)?;
    file.write(&kbuf)
}

pub fn sys_pread64(fd: i32, buf: usize, len: usize, off: u64) -> SysResult {
    let file = get_file(fd)?;
    let chunk = len.min(1024 * 1024);
    let mut kbuf = alloc::vec![0u8; chunk];
    let n = file.pread(&mut kbuf, off)?;
    copy_to_user(buf, &kbuf[..n])?;
    Ok(n)
}

pub fn sys_pwrite64(fd: i32, buf: usize, len: usize, off: u64) -> SysResult {
    let file = get_file(fd)?;
    let kbuf = read_bytes(buf, len.min(1024 * 1024))?;
    file.pwrite(&kbuf, off)
}

fn read_iovecs(iov: usize, cnt: usize) -> Result<Vec<Iovec>, i32> {
    if cnt > 1024 {
        return Err(EINVAL);
    }
    let mut v = Vec::with_capacity(cnt);
    for i in 0..cnt {
        let e: Iovec = read_val(iov + i * 16)?;
        v.push(e);
    }
    Ok(v)
}

pub fn sys_readv(fd: i32, iov: usize, cnt: usize) -> SysResult {
    let file = get_file(fd)?;
    let iovs = read_iovecs(iov, cnt)?;
    let total: usize = iovs.iter().map(|v| v.len).sum::<usize>().min(256 * 1024);
    if total == 0 {
        return Ok(0);
    }
    let mut kbuf = alloc::vec![0u8; total];
    let n = file.read(&mut kbuf)?;
    let mut done = 0;
    for v in &iovs {
        if done >= n {
            break;
        }
        let take = v.len.min(n - done);
        copy_to_user(v.base, &kbuf[done..done + take])?;
        done += take;
    }
    Ok(n)
}

fn gather(iovs: &[Iovec], max: usize) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    for v in iovs {
        if out.len() >= max {
            break;
        }
        let take = v.len.min(max - out.len());
        let start = out.len();
        out.resize(start + take, 0);
        copy_from_user(&mut out[start..], v.base)?;
    }
    Ok(out)
}

pub fn sys_writev(fd: i32, iov: usize, cnt: usize) -> SysResult {
    let file = get_file(fd)?;
    let iovs = read_iovecs(iov, cnt)?;
    let data = gather(&iovs, 1024 * 1024)?;
    if data.is_empty() {
        return Ok(0);
    }
    file.write(&data)
}

pub fn sys_preadv(fd: i32, iov: usize, cnt: usize, off: u64) -> SysResult {
    let file = get_file(fd)?;
    let iovs = read_iovecs(iov, cnt)?;
    let total: usize = iovs.iter().map(|v| v.len).sum::<usize>().min(1024 * 1024);
    let mut kbuf = alloc::vec![0u8; total];
    let n = file.pread(&mut kbuf, off)?;
    let mut done = 0;
    for v in &iovs {
        if done >= n {
            break;
        }
        let take = v.len.min(n - done);
        copy_to_user(v.base, &kbuf[done..done + take])?;
        done += take;
    }
    Ok(n)
}

pub fn sys_pwritev(fd: i32, iov: usize, cnt: usize, off: u64) -> SysResult {
    let file = get_file(fd)?;
    let iovs = read_iovecs(iov, cnt)?;
    let data = gather(&iovs, 1024 * 1024)?;
    file.pwrite(&data, off)
}

pub fn sys_sendfile(out_fd: i32, in_fd: i32, off_ptr: usize, count: usize) -> SysResult {
    let out = get_file(out_fd)?;
    let inf = get_file(in_fd)?;
    if !inf.ops.seekable() {
        return Err(EINVAL);
    }
    let mut off: u64 = if off_ptr != 0 { read_val::<i64>(off_ptr)? as u64 } else { *inf.pos.lock() };
    let mut total = 0usize;
    let mut buf = alloc::vec![0u8; 64 * 1024];
    while total < count {
        let want = (count - total).min(buf.len());
        let n = inf.pread(&mut buf[..want], off)?;
        if n == 0 {
            break;
        }
        let mut written = 0;
        while written < n {
            match out.write(&buf[written..n]) {
                Ok(0) => break,
                Ok(w) => written += w,
                Err(EAGAIN) if total + written > 0 => break,
                Err(e) => {
                    if total + written > 0 {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        off += written as u64;
        total += written;
        if written < n {
            break;
        }
    }
    if off_ptr != 0 {
        write_val(off_ptr, off as i64)?;
    } else {
        *inf.pos.lock() = off;
    }
    Ok(total)
}

pub fn sys_copy_file_range(fd_in: i32, off_in: usize, fd_out: i32, off_out: usize, len: usize) -> SysResult {
    let inf = get_file(fd_in)?;
    let out = get_file(fd_out)?;
    let mut ioff: u64 = if off_in != 0 { read_val::<i64>(off_in)? as u64 } else { *inf.pos.lock() };
    let mut ooff: u64 = if off_out != 0 { read_val::<i64>(off_out)? as u64 } else { *out.pos.lock() };
    let mut buf = alloc::vec![0u8; len.min(64 * 1024)];
    let mut total = 0;
    while total < len {
        let want = (len - total).min(buf.len());
        let n = inf.pread(&mut buf[..want], ioff)?;
        if n == 0 {
            break;
        }
        let w = out.pwrite(&buf[..n], ooff)?;
        ioff += w as u64;
        ooff += w as u64;
        total += w;
        if w < n {
            break;
        }
    }
    if off_in != 0 {
        write_val(off_in, ioff as i64)?;
    } else {
        *inf.pos.lock() = ioff;
    }
    if off_out != 0 {
        write_val(off_out, ooff as i64)?;
    } else {
        *out.pos.lock() = ooff;
    }
    Ok(total)
}

pub fn sys_lseek(fd: i32, off: i64, whence: i32) -> SysResult {
    let file = get_file(fd)?;
    file.lseek(off, whence).map(|v| v as usize)
}

pub fn sys_fstat(fd: i32, statbuf: usize) -> SysResult {
    let file = get_file(fd)?;
    let st = file.stat()?;
    write_val(statbuf, st)?;
    Ok(0)
}

pub fn sys_fstatat(dirfd: i32, path: usize, statbuf: usize, flags: u32) -> SysResult {
    let path = read_path(path)?;
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let d = lookup_at(dirfd, &path, follow, flags)?;
    write_val(statbuf, d.stat())?;
    Ok(0)
}

fn stat_to_statx(st: &Stat) -> Statx {
    Statx {
        stx_mask: 0x7ff,
        stx_blksize: st.st_blksize as u32,
        stx_attributes: 0,
        stx_nlink: st.st_nlink,
        stx_uid: st.st_uid,
        stx_gid: st.st_gid,
        stx_mode: st.st_mode as u16,
        __spare0: 0,
        stx_ino: st.st_ino,
        stx_size: st.st_size as u64,
        stx_blocks: st.st_blocks as u64,
        stx_attributes_mask: 0,
        stx_atime: StatxTimestamp { tv_sec: st.st_atime, tv_nsec: st.st_atime_nsec as u32, __reserved: 0 },
        stx_btime: StatxTimestamp { tv_sec: st.st_ctime, tv_nsec: 0, __reserved: 0 },
        stx_ctime: StatxTimestamp { tv_sec: st.st_ctime, tv_nsec: st.st_ctime_nsec as u32, __reserved: 0 },
        stx_mtime: StatxTimestamp { tv_sec: st.st_mtime, tv_nsec: st.st_mtime_nsec as u32, __reserved: 0 },
        stx_rdev_major: (st.st_rdev >> 8) as u32,
        stx_rdev_minor: (st.st_rdev & 0xff) as u32,
        stx_dev_major: 0,
        stx_dev_minor: st.st_dev as u32,
        stx_mnt_id: 1,
        stx_dio_mem_align: 0,
        stx_dio_offset_align: 0,
        __spare3: [0; 12],
    }
}

pub fn sys_statx(dirfd: i32, path: usize, flags: u32, _mask: u32, buf: usize) -> SysResult {
    let path = read_path(path)?;
    let st = if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        get_file(dirfd)?.stat()?
    } else {
        let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
        lookup_at(dirfd, &path, follow, flags)?.stat()
    };
    write_val(buf, stat_to_statx(&st))?;
    Ok(0)
}

pub fn sys_faccessat(dirfd: i32, path: usize, _mode: u32, flags: u32) -> SysResult {
    let path = read_path(path)?;
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    lookup_at(dirfd, &path, follow, flags)?;
    Ok(0)
}

pub fn sys_readlinkat(dirfd: i32, path: usize, buf: usize, bufsz: usize) -> SysResult {
    let path = read_path(path)?;
    if path.is_empty() || path == "/proc/self/exe" {
        let exe = current().inner.lock().exe_path.clone();
        let b = exe.as_bytes();
        let n = b.len().min(bufsz);
        copy_to_user(buf, &b[..n])?;
        return Ok(n);
    }
    let d = lookup_at(dirfd, &path, false, 0)?;
    match &d.kind {
        NodeKind::Symlink(t) => {
            let b = t.as_bytes();
            let n = b.len().min(bufsz);
            copy_to_user(buf, &b[..n])?;
            Ok(n)
        }
        _ => Err(EINVAL),
    }
}

pub fn sys_getcwd(buf: usize, size: usize) -> SysResult {
    let cwd = current().cwd();
    let p = cwd.path();
    let b = p.as_bytes();
    if b.len() + 1 > size {
        return Err(ERANGE);
    }
    copy_to_user(buf, b)?;
    copy_to_user(buf + b.len(), &[0])?;
    Ok(b.len() + 1)
}

pub fn sys_chdir(path: usize) -> SysResult {
    let path = read_path(path)?;
    let d = vfs::lookup(&current().cwd(), &path, true)?;
    if !d.is_dir() {
        return Err(ENOTDIR);
    }
    current().inner.lock().cwd = d;
    Ok(0)
}

pub fn sys_fchdir(fd: i32) -> SysResult {
    let d = get_file(fd)?.dentry().ok_or(ENOTDIR)?;
    if !d.is_dir() {
        return Err(ENOTDIR);
    }
    current().inner.lock().cwd = d;
    Ok(0)
}

pub fn sys_mkdirat(dirfd: i32, path: usize, mode: u32) -> SysResult {
    let path = read_path(path)?;
    let base = dir_base(dirfd)?;
    let (parent, name) = vfs::lookup_parent(&base, &path)?;
    if parent.lookup_child(&name).is_some() {
        return Err(EEXIST);
    }
    let umask = current().inner.lock().umask;
    let d = Dentry::new_dir(&name, Arc::downgrade(&parent), mode & !umask);
    parent.add_child(d)?;
    Ok(0)
}

pub fn sys_mknodat(dirfd: i32, path: usize, mode: u32, _dev: usize) -> SysResult {
    let path = read_path(path)?;
    let base = dir_base(dirfd)?;
    let (parent, name) = vfs::lookup_parent(&base, &path)?;
    if parent.lookup_child(&name).is_some() {
        return Err(EEXIST);
    }
    let kind = match mode & S_IFMT {
        S_IFREG | 0 => NodeKind::File(crate::sync::SpinLock::new(Vec::new())),
        S_IFIFO => NodeKind::Fifo,
        S_IFSOCK => NodeKind::Socket,
        _ => return Err(EPERM),
    };
    let m = if mode & S_IFMT == 0 { S_IFREG | mode } else { mode };
    let d = Dentry::new(&name, Arc::downgrade(&parent), m, kind);
    parent.add_child(d)?;
    Ok(0)
}

pub fn sys_unlinkat(dirfd: i32, path: usize, flags: u32) -> SysResult {
    let path = read_path(path)?;
    let base = dir_base(dirfd)?;
    let (parent, name) = vfs::lookup_parent(&base, &path)?;
    let d = parent.lookup_child(&name).ok_or(ENOENT)?;
    if flags & AT_REMOVEDIR != 0 {
        if !d.is_dir() {
            return Err(ENOTDIR);
        }
        if !d.children().is_empty() {
            return Err(ENOTEMPTY);
        }
    } else if d.is_dir() {
        return Err(EISDIR);
    }
    parent.remove_child(&name)?;
    Ok(0)
}

pub fn sys_symlinkat(target: usize, dirfd: i32, linkpath: usize) -> SysResult {
    let target = read_path(target)?;
    let linkpath = read_path(linkpath)?;
    let base = dir_base(dirfd)?;
    let (parent, name) = vfs::lookup_parent(&base, &linkpath)?;
    if parent.lookup_child(&name).is_some() {
        return Err(EEXIST);
    }
    let d = Dentry::new(&name, Arc::downgrade(&parent), S_IFLNK | 0o777, NodeKind::Symlink(target));
    parent.add_child(d)?;
    Ok(0)
}

pub fn sys_linkat(olddirfd: i32, oldpath: usize, newdirfd: i32, newpath: usize, flags: u32) -> SysResult {
    // Hard links are emulated by copying file contents (ramfs has no shared inodes).
    let oldpath = read_path(oldpath)?;
    let newpath = read_path(newpath)?;
    let old = lookup_at(olddirfd, &oldpath, flags & AT_SYMLINK_FOLLOW != 0, flags)?;
    let base = dir_base(newdirfd)?;
    let (parent, name) = vfs::lookup_parent(&base, &newpath)?;
    if parent.lookup_child(&name).is_some() {
        return Err(EEXIST);
    }
    let NodeKind::File(data) = &old.kind else { return Err(EPERM) };
    let copy = data.lock().clone();
    let d = Dentry::new_file(&name, Arc::downgrade(&parent), old.mode(), copy);
    parent.add_child(d)?;
    Ok(0)
}

pub fn sys_renameat(olddirfd: i32, oldpath: usize, newdirfd: i32, newpath: usize) -> SysResult {
    let oldpath = read_path(oldpath)?;
    let newpath = read_path(newpath)?;
    let obase = dir_base(olddirfd)?;
    let nbase = dir_base(newdirfd)?;
    let (oparent, oname) = vfs::lookup_parent(&obase, &oldpath)?;
    let (nparent, nname) = vfs::lookup_parent(&nbase, &newpath)?;
    let d = oparent.lookup_child(&oname).ok_or(ENOENT)?;
    if let Some(existing) = nparent.lookup_child(&nname) {
        if Arc::ptr_eq(&existing, &d) {
            return Ok(0);
        }
        if existing.is_dir() && !existing.children().is_empty() {
            return Err(ENOTEMPTY);
        }
        nparent.remove_child(&nname)?;
    }
    oparent.remove_child(&oname)?;
    *d.name.lock() = nname.clone();
    nparent.add_child(d)?;
    Ok(0)
}

pub fn sys_truncate(path: usize, len: i64) -> SysResult {
    let path = read_path(path)?;
    let d = vfs::lookup(&current().cwd(), &path, true)?;
    match &d.kind {
        NodeKind::File(data) => {
            data.lock().resize(len.max(0) as usize, 0);
            Ok(0)
        }
        NodeKind::Dir(_) => Err(EISDIR),
        _ => Err(EINVAL),
    }
}

pub fn sys_ftruncate(fd: i32, len: i64) -> SysResult {
    let file = get_file(fd)?;
    if len < 0 {
        return Err(EINVAL);
    }
    file.ops.truncate(len as u64)?;
    Ok(0)
}

pub fn sys_fallocate(fd: i32, _mode: i32, off: i64, len: i64) -> SysResult {
    let file = get_file(fd)?;
    let end = (off + len) as u64;
    if file.ops.size() < end {
        file.ops.truncate(end)?;
    }
    Ok(0)
}

pub fn sys_fchmod(fd: i32, mode: u32) -> SysResult {
    let file = get_file(fd)?;
    if let Some(d) = file.dentry() {
        let mut m = d.meta.lock();
        m.mode = (m.mode & S_IFMT) | (mode & 0o7777);
    }
    Ok(0)
}

pub fn sys_fchmodat(dirfd: i32, path: usize, mode: u32) -> SysResult {
    let path = read_path(path)?;
    let d = lookup_at(dirfd, &path, true, 0)?;
    let mut m = d.meta.lock();
    m.mode = (m.mode & S_IFMT) | (mode & 0o7777);
    Ok(0)
}

pub fn sys_fchownat(dirfd: i32, path: usize, uid: u32, gid: u32, flags: u32) -> SysResult {
    let path = read_path(path)?;
    let d = lookup_at(dirfd, &path, flags & AT_SYMLINK_NOFOLLOW == 0, flags)?;
    let mut m = d.meta.lock();
    if uid != u32::MAX {
        m.uid = uid;
    }
    if gid != u32::MAX {
        m.gid = gid;
    }
    Ok(0)
}

pub fn sys_fchown(fd: i32, uid: u32, gid: u32) -> SysResult {
    let file = get_file(fd)?;
    if let Some(d) = file.dentry() {
        let mut m = d.meta.lock();
        if uid != u32::MAX {
            m.uid = uid;
        }
        if gid != u32::MAX {
            m.gid = gid;
        }
    }
    Ok(0)
}

pub fn sys_utimensat(dirfd: i32, path: usize, times: usize, flags: u32) -> SysResult {
    let d = if path == 0 {
        get_file(dirfd)?.dentry().ok_or(EBADF)?
    } else {
        let p = read_path(path)?;
        lookup_at(dirfd, &p, flags & AT_SYMLINK_NOFOLLOW == 0, flags)?
    };
    let now = vfs::now_secs();
    let (at, mt) = if times == 0 {
        (now, now)
    } else {
        let a: Timespec = read_val(times)?;
        let m: Timespec = read_val(times + 16)?;
        let conv = |t: Timespec, cur: i64| -> i64 {
            match t.tv_nsec {
                0x3fffffff => now, // UTIME_NOW
                0x3ffffffe => cur, // UTIME_OMIT
                _ => t.tv_sec,
            }
        };
        let m0 = *d.meta.lock();
        (conv(a, m0.atime), conv(m, m0.mtime))
    };
    let mut m = d.meta.lock();
    m.atime = at;
    m.mtime = mt;
    m.ctime = now;
    Ok(0)
}

pub fn sys_umask(mask: u32) -> SysResult {
    let cur = current();
    let mut inner = cur.inner.lock();
    let old = inner.umask;
    inner.umask = mask & 0o777;
    Ok(old as usize)
}

pub fn sys_getdents64(fd: i32, buf: usize, len: usize) -> SysResult {
    let file = get_file(fd)?;
    let entries = file.ops.readdir()?;
    let mut pos = *file.pos.lock() as usize;
    let mut out: Vec<u8> = Vec::new();
    while pos < entries.len() {
        let e = &entries[pos];
        let name = e.name.as_bytes();
        let reclen = (19 + name.len() + 1 + 7) & !7;
        if out.len() + reclen > len {
            if out.is_empty() {
                return Err(EINVAL);
            }
            break;
        }
        let hdr = Dirent64Hdr { d_ino: e.ino, d_off: (pos + 1) as i64, d_reclen: reclen as u16, d_type: e.dtype };
        let hb = unsafe { core::slice::from_raw_parts(&hdr as *const _ as *const u8, 19) };
        out.extend_from_slice(hb);
        out.extend_from_slice(name);
        out.push(0);
        while out.len() % 8 != 0 {
            out.push(0);
        }
        pos += 1;
    }
    copy_to_user(buf, &out)?;
    *file.pos.lock() = pos as u64;
    Ok(out.len())
}

pub fn sys_dup(fd: i32) -> SysResult {
    let file = get_file(fd)?;
    install_fd(file, false)
}

pub fn sys_dup3(oldfd: i32, newfd: i32, flags: u32) -> SysResult {
    if oldfd == newfd {
        return Err(EINVAL);
    }
    let file = get_file(oldfd)?;
    let fds = current().fds();
    let mut t = fds.lock();
    let old = t.close(newfd).ok();
    t.set(newfd, file, flags & O_CLOEXEC != 0)?;
    drop(t);
    drop(old);
    Ok(newfd as usize)
}

pub fn sys_fcntl(fd: i32, cmd: u32, arg: usize) -> SysResult {
    let cur = current();
    let fds = cur.fds();
    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let file = fds.lock().get(fd)?;
            let nfd = fds.lock().alloc(file, cmd == F_DUPFD_CLOEXEC, arg)?;
            Ok(nfd as usize)
        }
        F_GETFD => {
            let t = fds.lock();
            let e = t.get_entry(fd)?;
            Ok(if e.cloexec { FD_CLOEXEC as usize } else { 0 })
        }
        F_SETFD => {
            fds.lock().set_cloexec(fd, arg as u32 & FD_CLOEXEC != 0)?;
            Ok(0)
        }
        F_GETFL => {
            let file = fds.lock().get(fd)?;
            Ok(file.flags() as usize)
        }
        F_SETFL => {
            let file = fds.lock().get(fd)?;
            let keep = file.flags() & (O_ACCMODE | O_PATH);
            let set = arg as u32 & (O_APPEND | O_NONBLOCK | O_DSYNC | O_DIRECT | O_NOATIME);
            file.set_flags(keep | set);
            Ok(0)
        }
        F_GETLK => {
            // no locks held: report F_UNLCK (type 2 at offset 0)
            write_val(arg, 2i16)?;
            Ok(0)
        }
        F_SETLK | F_SETLKW => {
            fds.lock().get(fd)?;
            Ok(0)
        }
        F_SETOWN => Ok(0),
        F_GETOWN => Ok(0),
        1024 | 1025 | 1026 => Ok(0), // F_SETLEASE etc
        1031 | 1032 => {
            // F_SETPIPE_SZ / F_GETPIPE_SZ
            Ok(crate::fs::pipe::PIPE_BUF_SIZE)
        }
        _ => Err(EINVAL),
    }
}

pub fn sys_ioctl(fd: i32, cmd: u32, arg: usize) -> SysResult {
    let file = get_file(fd)?;
    match cmd {
        FIONBIO => {
            let on: i32 = read_val(arg)?;
            let f = file.flags();
            file.set_flags(if on != 0 { f | O_NONBLOCK } else { f & !O_NONBLOCK });
            Ok(0)
        }
        FIOCLEX => {
            current().fds().lock().set_cloexec(fd, true)?;
            Ok(0)
        }
        FIONCLEX => {
            current().fds().lock().set_cloexec(fd, false)?;
            Ok(0)
        }
        _ => file.ops.ioctl(cmd, arg),
    }
}

pub fn sys_pipe2(fds_ptr: usize, flags: u32) -> SysResult {
    let (r, w) = create_pipe();
    let fl = flags & O_NONBLOCK;
    let rf = File::new(r, O_RDONLY | fl, String::from("pipe:[r]"));
    let wf = File::new(w, O_WRONLY | fl, String::from("pipe:[w]"));
    let cloexec = flags & O_CLOEXEC != 0;
    let fds = current().fds();
    let mut t = fds.lock();
    let rfd = t.alloc(rf, cloexec, 0)?;
    let wfd = match t.alloc(wf, cloexec, 0) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = t.close(rfd);
            return Err(e);
        }
    };
    drop(t);
    write_val(fds_ptr, [rfd, wfd])?;
    Ok(0)
}

pub fn sys_eventfd2(init: u32, flags: u32) -> SysResult {
    let ops: Arc<dyn FileOps> = Arc::new(EventFd::new(init as u64, flags & EFD_SEMAPHORE != 0));
    let f = File::new(ops, O_RDWR | (flags & O_NONBLOCK), String::from("anon_inode:[eventfd]"));
    install_fd(f, flags & O_CLOEXEC != 0)
}

pub fn sys_epoll_create1(flags: u32) -> SysResult {
    let ep: Arc<dyn FileOps> = Epoll::new();
    let f = File::new(ep, O_RDWR, String::from("anon_inode:[eventpoll]"));
    install_fd(f, flags & O_CLOEXEC != 0)
}

fn get_epoll(fd: i32) -> Result<(Arc<File>, Arc<Epoll>), i32> {
    let file = get_file(fd)?;
    let ep = file.ops.clone();
    let any = ep.as_any();
    if any.downcast_ref::<Epoll>().is_none() {
        return Err(EINVAL);
    }
    // Recover an Arc<Epoll> from the Arc<dyn FileOps>.
    let ptr = Arc::into_raw(ep) as *const Epoll;
    let ep = unsafe { Arc::from_raw(ptr) };
    Ok((file, ep))
}

pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event: usize) -> SysResult {
    let (_f, ep) = get_epoll(epfd)?;
    let target = get_file(fd).ok();
    if target.is_none() {
        return Err(EBADF);
    }
    let ev = if op == EPOLL_CTL_DEL { EpollEvent::default() } else { read_val(event)? };
    ep.ctl(op, fd, target, ev)?;
    Ok(0)
}

pub fn sys_epoll_pwait(epfd: i32, events: usize, maxevents: i32, timeout_ms: i32, _sigmask: usize) -> SysResult {
    if maxevents <= 0 {
        return Err(EINVAL);
    }
    let (_f, ep) = get_epoll(epfd)?;
    let deadline = if timeout_ms < 0 { None } else { Some(monotonic_ns() + timeout_ms as u64 * 1_000_000) };
    let evs = ep.wait(maxevents as usize, deadline)?;
    for (i, e) in evs.iter().enumerate() {
        write_val(events + i * 16, *e)?;
    }
    Ok(evs.len())
}

pub fn sys_epoll_pwait2(epfd: i32, events: usize, maxevents: i32, timeout: usize, _sigmask: usize) -> SysResult {
    let ms = if timeout == 0 {
        -1
    } else {
        let ts: Timespec = read_val(timeout)?;
        (ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000) as i32
    };
    sys_epoll_pwait(epfd, events, maxevents, ms, 0)
}

fn poll_events_to_mask(ev: u32) -> u32 {
    ev
}

pub fn sys_ppoll(fds: usize, nfds: usize, tsp: usize, _sigmask: usize) -> SysResult {
    if nfds > 4096 {
        return Err(EINVAL);
    }
    let mut pfds: Vec<PollFd> = Vec::with_capacity(nfds);
    for i in 0..nfds {
        pfds.push(read_val(fds + i * 8)?);
    }
    let deadline = if tsp == 0 {
        None
    } else {
        let ts: Timespec = read_val(tsp)?;
        Some(monotonic_ns() + (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64)
    };
    let cur = current();
    let files: Vec<Option<Arc<File>>> = pfds.iter().map(|p| if p.fd < 0 { None } else { get_file(p.fd).ok() }).collect();
    loop {
        crate::net::poll();
        let mut count = 0;
        for (i, p) in pfds.iter_mut().enumerate() {
            p.revents = 0;
            if p.fd < 0 {
                continue;
            }
            match &files[i] {
                None => {
                    p.revents = POLLNVAL as i16;
                    count += 1;
                }
                Some(f) => {
                    let ready = f.poll();
                    let want = (p.events as u16 as u32) | POLLERR | POLLHUP;
                    let r = poll_events_to_mask(ready) & want;
                    if r != 0 {
                        p.revents = r as i16;
                        count += 1;
                    }
                }
            }
        }
        if count > 0 {
            for (i, p) in pfds.iter().enumerate() {
                write_val(fds + i * 8, *p)?;
            }
            return Ok(count);
        }
        if let Some(d) = deadline {
            if monotonic_ns() >= d {
                for (i, p) in pfds.iter().enumerate() {
                    write_val(fds + i * 8, *p)?;
                }
                return Ok(0);
            }
        }
        if crate::task::signal::has_deliverable(&cur) {
            return Err(EINTR);
        }
        for f in files.iter().flatten() {
            if let Some(wq) = f.ops.wait_queue() {
                wq.add(&cur);
            }
        }
        if let Some(d) = deadline {
            crate::time::add_sleeper(&cur, d);
        }
        crate::task::sched::block_current();
        if deadline.is_some() {
            crate::time::remove_sleeper(&cur);
        }
        for f in files.iter().flatten() {
            if let Some(wq) = f.ops.wait_queue() {
                wq.remove(&cur);
            }
        }
    }
}

pub fn sys_pselect6(nfds: i32, readfds: usize, writefds: usize, exceptfds: usize, tsp: usize, _sigmask: usize) -> SysResult {
    if nfds < 0 || nfds > 1024 {
        return Err(EINVAL);
    }
    let nwords = (nfds as usize + 63) / 64;
    let read_set = |p: usize| -> Result<Vec<u64>, i32> {
        let mut v = alloc::vec![0u64; nwords];
        if p != 0 {
            for i in 0..nwords {
                v[i] = read_val(p + i * 8)?;
            }
        }
        Ok(v)
    };
    let rset = read_set(readfds)?;
    let wset = read_set(writefds)?;
    let eset = read_set(exceptfds)?;
    let deadline = if tsp == 0 {
        None
    } else {
        let ts: Timespec = read_val(tsp)?;
        Some(monotonic_ns() + (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64)
    };
    let cur = current();
    let isset = |s: &[u64], fd: usize| s[fd / 64] & (1 << (fd % 64)) != 0;
    let mut files: Vec<(usize, Arc<File>, bool, bool)> = Vec::new();
    for fd in 0..nfds as usize {
        let r = isset(&rset, fd);
        let w = isset(&wset, fd);
        let e = isset(&eset, fd);
        if r || w || e {
            let f = get_file(fd as i32)?;
            files.push((fd, f, r, w));
        }
    }
    loop {
        crate::net::poll();
        let mut rout = alloc::vec![0u64; nwords];
        let mut wout = alloc::vec![0u64; nwords];
        let mut count = 0;
        for (fd, f, r, w) in &files {
            let ready = f.poll();
            if *r && ready & (POLLIN | POLLHUP | POLLERR) != 0 {
                rout[fd / 64] |= 1 << (fd % 64);
                count += 1;
            }
            if *w && ready & (POLLOUT | POLLERR) != 0 {
                wout[fd / 64] |= 1 << (fd % 64);
                count += 1;
            }
        }
        let timed_out = deadline.map(|d| monotonic_ns() >= d).unwrap_or(false);
        if count > 0 || timed_out {
            for i in 0..nwords {
                if readfds != 0 {
                    write_val(readfds + i * 8, rout[i])?;
                }
                if writefds != 0 {
                    write_val(writefds + i * 8, wout[i])?;
                }
                if exceptfds != 0 {
                    write_val(exceptfds + i * 8, 0u64)?;
                }
            }
            return Ok(count);
        }
        if crate::task::signal::has_deliverable(&cur) {
            return Err(EINTR);
        }
        for (_, f, _, _) in &files {
            if let Some(wq) = f.ops.wait_queue() {
                wq.add(&cur);
            }
        }
        if let Some(d) = deadline {
            crate::time::add_sleeper(&cur, d);
        }
        crate::task::sched::block_current();
        if deadline.is_some() {
            crate::time::remove_sleeper(&cur);
        }
        for (_, f, _, _) in &files {
            if let Some(wq) = f.ops.wait_queue() {
                wq.remove(&cur);
            }
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StatFs {
    f_type: i64,
    f_bsize: i64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [i32; 2],
    f_namelen: i64,
    f_frsize: i64,
    f_flags: i64,
    f_spare: [i64; 4],
}

fn statfs_val() -> StatFs {
    let (used, total) = crate::mm::heap::stats();
    let bsize = 4096u64;
    StatFs {
        f_type: 0x01021994, // TMPFS_MAGIC
        f_bsize: bsize as i64,
        f_blocks: total as u64 / bsize,
        f_bfree: (total - used) as u64 / bsize,
        f_bavail: (total - used) as u64 / bsize,
        f_files: 1 << 20,
        f_ffree: 1 << 19,
        f_fsid: [0, 0],
        f_namelen: 255,
        f_frsize: bsize as i64,
        f_flags: 0,
        f_spare: [0; 4],
    }
}

pub fn sys_statfs(path: usize, buf: usize) -> SysResult {
    let path = read_path(path)?;
    vfs::lookup(&current().cwd(), &path, true)?;
    write_val(buf, statfs_val())?;
    Ok(0)
}

pub fn sys_fstatfs(fd: i32, buf: usize) -> SysResult {
    get_file(fd)?;
    write_val(buf, statfs_val())?;
    Ok(0)
}

pub fn sys_memfd_create(name: usize, flags: u32) -> SysResult {
    let name = read_string(name, 249)?;
    let d = Dentry::new_file(&name, alloc::sync::Weak::new(), 0o600, Vec::new());
    let f = crate::fs::open_dentry(d, O_RDWR)?;
    install_fd(f, flags & 1 != 0)
}
