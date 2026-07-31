//! File-related syscalls.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs::{self, Fd, FdKind, FdTable};
use crate::syscall::{read_cstr, read_user, write_user};
use crate::task;

fn cur_fds() -> *mut FdTable {
    let t = task::current();
    unsafe { &mut t.as_mut().unwrap().fds as *mut FdTable }
}

fn get_fd(fd: usize) -> Option<&'static mut Fd> {
    unsafe { (&mut *cur_fds()).get_mut(fd) }
}

pub fn sys_read(fd: usize, buf: usize, len: usize) -> isize {
    if len > 1 << 30 {
        return -22;
    }
    let mut v = vec![0u8; len];
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match fs::read_fd(f, &mut v) {
        Ok(n) => {
            if n > 0 {
                let _ = write_user(buf, &v[..n]);
            }
            n as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_write(fd: usize, buf: usize, len: usize) -> isize {
    let data = match read_user(buf, len) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match fs::write_fd(f, &data) {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

fn read_iovecs(iov: usize, iovcnt: usize) -> Result<Vec<(usize, usize)>, i32> {
    let mut out = Vec::new();
    for i in 0..iovcnt {
        let d = read_user(iov + i * 16, 16)?;
        let base = u64::from_le_bytes(d[..8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(d[8..].try_into().unwrap()) as usize;
        out.push((base, len));
    }
    Ok(out)
}

pub fn sys_readv(fd: usize, iov: usize, iovcnt: usize) -> isize {
    let iovs = match read_iovecs(iov, iovcnt) {
        Ok(v) => v,
        Err(e) => return e as isize,
    };
    let mut total = 0usize;
    for (base, len) in iovs {
        let mut v = vec![0u8; len];
        let f = match get_fd(fd) {
            Some(f) => f,
            None => return -9,
        };
        match fs::read_fd(f, &mut v) {
            Ok(n) => {
                if n > 0 {
                    let _ = write_user(base, &v[..n]);
                    total += n;
                }
                if n < len {
                    break;
                }
            }
            Err(e) => return e as isize,
        }
    }
    total as isize
}

pub fn sys_writev(fd: usize, iov: usize, iovcnt: usize) -> isize {
    let iovs = match read_iovecs(iov, iovcnt) {
        Ok(v) => v,
        Err(e) => return e as isize,
    };
    let mut total = 0usize;
    for (base, len) in iovs {
        let data = match read_user(base, len) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        let f = match get_fd(fd) {
            Some(f) => f,
            None => return -9,
        };
        match fs::write_fd(f, &data) {
            Ok(n) => total += n,
            Err(e) => return e as isize,
        }
    }
    total as isize
}

pub fn sys_pread64(fd: usize, buf: usize, len: usize, off: usize) -> isize {
    let mut v = vec![0u8; len];
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let saved = f.offset;
    f.offset = off as u64;
    let r = fs::read_fd(f, &mut v);
    f.offset = saved;
    match r {
        Ok(n) => {
            if n > 0 {
                let _ = write_user(buf, &v[..n]);
            }
            n as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_pwrite64(fd: usize, buf: usize, len: usize, off: usize) -> isize {
    let data = match read_user(buf, len) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let saved = f.offset;
    f.offset = off as u64;
    let r = fs::write_fd(f, &data);
    f.offset = saved;
    match r {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

fn resolve_path_at(dirfd: isize, path: &str) -> String {
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    if dirfd == fs::AT_FDCWD {
        if path.starts_with('/') {
            path.to_string()
        } else {
            let joined = if cwd == "/" {
                format!("/{}", path)
            } else {
                format!("{}/{}", cwd, path)
            };
            joined
        }
    } else {
        // dirfd not supported except AT_FDCWD
        path.to_string()
    }
}

pub fn sys_openat(dirfd: isize, path: usize, flags: usize, mode: usize) -> isize {
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let resolved = resolve_path_at(dirfd, &path_str);
    let flags = flags as u32;
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };

    let file_id = fs::resolve(&cwd, &resolved);
    let file_id = match file_id {
        Some(id) => {
            // check dir
            let f = fs::fs().get(id).unwrap();
            let is_dir = f.borrow().is_dir;
            if is_dir && flags & fs::O_DIRECTORY == 0 && flags & fs::O_WRONLY != 0 {
                return -21; // EISDIR
            }
            if is_dir && (flags & fs::O_WRONLY != 0) {
                return -21;
            }
            if flags & fs::O_TRUNC != 0 && !is_dir {
                f.borrow_mut().data.clear();
            }
            if flags & fs::O_EXCL != 0 && flags & fs::O_CREAT != 0 {
                return -17; // EEXIST
            }
            Some(id)
        }
        None => {
            if flags & fs::O_CREAT != 0 {
                match fs::create_at(&cwd, &resolved, mode as u32) {
                    Ok(id) => Some(id),
                    Err(e) => return e as isize,
                }
            } else {
                None
            }
        }
    };
    let id = match file_id {
        Some(id) => id,
        None => return -2, // ENOENT
    };
    // alloc fd
    let fds = unsafe { &mut *cur_fds() };
    let fdnum = match fds.alloc() {
        Some(fd) => fd,
        None => return -24, // EMFILE
    };
    fds.fds[fdnum] = Some(Fd {
        kind: FdKind::File { file_id: id },
        flags,
        offset: 0,
        cloexec: flags & fs::O_CLOEXEC != 0,
        epoll: None,
    });
    fdnum as isize
}

pub fn sys_close(fd: usize) -> isize {
    let fds = unsafe { &mut *cur_fds() };
    if !fds.close(fd) {
        return -9;
    }
    // if this fd had an epoll interest, remove it
    crate::epoll::fd_removed(fd);
    0
}

pub fn sys_lseek(fd: usize, off: i64, whence: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let size = match &f.kind {
        FdKind::File { file_id } => {
            let file = fs::fs().get(*file_id).unwrap();
            let len = file.borrow().data.len() as i64;
            len
        }
        _ => 0,
    };
    let new = match whence {
        0 => off,
        1 => f.offset as i64 + off,
        2 => size + off,
        _ => return -22,
    };
    if new < 0 {
        return -22;
    }
    f.offset = new as u64;
    new as isize
}

pub fn sys_dup(old: usize) -> isize {
    let fds = unsafe { &mut *cur_fds() };
    let f = match fds.get(old) {
        Some(f) => f.clone(),
        None => return -9,
    };
    let new = match fds.alloc() {
        Some(n) => n,
        None => return -24,
    };
    fds.fds[new] = Some(f);
    new as isize
}

pub fn sys_dup3(old: usize, new: usize, flags: usize) -> isize {
    if old == new {
        return -22;
    }
    let fds = unsafe { &mut *cur_fds() };
    let f = match fds.get(old) {
        Some(f) => f.clone(),
        None => return -9,
    };
    let mut f = f;
    if flags & fs::O_CLOEXEC as usize != 0 {
        f.cloexec = true;
    }
    // close old fd at new position
    if new < fds.fds.len() {
        fds.fds[new] = None;
    }
    fds.install(new, f);
    new as isize
}

pub fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match cmd {
        0 => {
            // F_DUPFD
            let f = f.clone();
            let fds = unsafe { &mut *cur_fds() };
            let mut n = arg;
            while n < fds.fds.len() && fds.fds[n].is_some() {
                n += 1;
            }
            fds.install(n, f);
            n as isize
        }
        1 => {
            // F_GETFD
            if f.cloexec {
                1
            } else {
                0
            }
        }
        2 => {
            // F_SETFD
            f.cloexec = arg & 1 != 0;
            0
        }
        3 => {
            // F_GETFL
            f.flags as isize
        }
        4 => {
            // F_SETFL (only O_NONBLOCK/O_APPEND bits)
            let keep = f.flags & !(fs::O_NONBLOCK | fs::O_APPEND);
            f.flags = keep | (arg as u32 & (fs::O_NONBLOCK | fs::O_APPEND));
            if let FdKind::Socket { sock_id } = f.kind {
                crate::net::sock(sock_id).unwrap().nonblock = arg & fs::O_NONBLOCK as usize != 0;
            }
            if let FdKind::UnixPair { sock_id } = f.kind {
                crate::net::sock(sock_id).unwrap().nonblock = arg & fs::O_NONBLOCK as usize != 0;
            }
            0
        }
        5 | 6 | 7 => {
            // F_GETLK / F_SETLK / F_SETLKW: no-op
            0
        }
        8 => {
            // F_SETOWN (riscv64 numbering)
            0
        }
        9 => {
            // F_GETOWN (riscv64 numbering)
            0
        }
        _ => -22,
    }
}

pub fn sys_ioctl(fd: usize, req: usize, arg: usize) -> isize {
    let _ = (fd, req, arg);
    // FIONBIO: arg points to int; set nonblock
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let _ = f;
    0
}

pub fn sys_fstat(fd: usize, buf: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let mut out = [0u8; 128];
    match fs::fill_stat(f, &mut out) {
        Ok(_) => match write_user(buf, &out) {
            Ok(_) => 0,
            Err(e) => e as isize,
        },
        Err(e) => e as isize,
    }
}

pub fn sys_newfstatat(dirfd: isize, path: usize, buf: usize, flags: usize) -> isize {
    let _ = flags;
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let resolved = resolve_path_at(dirfd, &path_str);
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    let mut out = [0u8; 128];
    match fs::fill_stat_path(&cwd, &resolved, &mut out) {
        Ok(_) => match write_user(buf, &out) {
            Ok(_) => 0,
            Err(e) => e as isize,
        },
        Err(e) => e as isize,
    }
}

pub fn sys_faccessat(dirfd: isize, path: usize, mode: usize) -> isize {
    let _ = (dirfd, mode);
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let resolved = resolve_path_at(dirfd, &path_str);
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    match fs::resolve(&cwd, &resolved) {
        Some(_) => 0,
        None => -2,
    }
}

pub fn sys_getdents64(fd: usize, buf: usize, len: usize) -> isize {
    let mut v = vec![0u8; len];
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match fs::getdents(f, &mut v) {
        Ok(n) => {
            if n > 0 {
                let _ = write_user(buf, &v[..n]);
            }
            n as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_chdir(path: usize) -> isize {
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    let resolved = resolve_path_at(fs::AT_FDCWD, &path_str);
    match fs::resolve(&cwd, &resolved) {
        Some(id) => {
            let f = fs::fs().get(id).unwrap();
            if !f.borrow().is_dir {
                return -20;
            }
            let t = task::current();
            unsafe {
                t.as_mut().unwrap().cwd = resolved;
            }
            0
        }
        None => -2,
    }
}

pub fn sys_fchdir(fd: usize) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let file_id = match &f.kind {
        FdKind::File { file_id } => *file_id,
        _ => return -20,
    };
    let file = fs::fs().get(file_id).unwrap();
    if !file.borrow().is_dir {
        return -20;
    }
    0
}

pub fn sys_getcwd(buf: usize, size: usize) -> isize {
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    let bytes = cwd.as_bytes();
    if bytes.len() + 1 > size {
        return -34; // ERANGE
    }
    let mut v = vec![0u8; bytes.len() + 1];
    v[..bytes.len()].copy_from_slice(bytes);
    match write_user(buf, &v) {
        Ok(_) => bytes.len() as isize + 1,
        Err(e) => e as isize,
    }
}

pub fn sys_mkdirat(dirfd: isize, path: usize, mode: usize) -> isize {
    let _ = dirfd;
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    match fs::mkdir_at(&cwd, &path_str, mode as u32) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_unlinkat(dirfd: isize, path: usize, flags: usize) -> isize {
    let _ = (dirfd, flags);
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    match fs::unlink_at(&cwd, &path_str) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_renameat(olddirfd: isize, oldpath: usize, newdirfd: isize, newpath: usize) -> isize {
    let _ = (olddirfd, newdirfd);
    let oldp = match read_cstr(oldpath, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let newp = match read_cstr(newpath, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    let resolved_old = resolve_path_at(fs::AT_FDCWD, &oldp);
    let resolved_new = resolve_path_at(fs::AT_FDCWD, &newp);
    let _ = (&cwd, &resolved_old, &resolved_new);
    // simple: remove new if exists, keep old (no real rename)
    let _ = fs::unlink_at(&cwd, &resolved_new);
    // move: recreate under new name
    if let Some(id) = fs::resolve(&cwd, &resolved_old) {
        let f = fs::fs().get(id).unwrap();
        let (data, is_dir, mode) = {
            let b = f.borrow();
            (b.data.clone(), b.is_dir, b.mode)
        };
        let _ = fs::unlink_at(&cwd, &resolved_old);
        if is_dir {
            let _ = fs::mkdir_at(&cwd, &resolved_new, mode & 0o7777);
        } else {
            let (parent, name) = fs::split_parent(&cwd, &resolved_new);
            if let Some(pid) = fs::resolve(&cwd, &parent) {
                fs::insert_file(pid, &name, fs::SharedFile::new_file(mode & 0o7777, data));
            }
        }
        0
    } else {
        -2
    }
}

pub fn sys_renameat2(olddirfd: isize, oldpath: usize, newdirfd: isize, newpath: usize, flags: usize) -> isize {
    let _ = flags;
    sys_renameat(olddirfd, oldpath, newdirfd, newpath)
}

pub fn sys_truncate(path: usize, len: i64) -> isize {
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let t = task::current();
    let cwd = unsafe { t.as_ref().unwrap().cwd.clone() };
    match fs::resolve(&cwd, &path_str) {
        Some(id) => {
            let f = fs::fs().get(id).unwrap();
            let mut file = f.borrow_mut();
            if len as usize > file.data.len() {
                file.data.resize(len as usize, 0);
            } else {
                file.data.truncate(len as usize);
            }
            0
        }
        None => -2,
    }
}

pub fn sys_ftruncate(fd: usize, len: i64) -> isize {
    let f = match get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match &f.kind {
        FdKind::File { file_id } => {
            let file = fs::fs().get(*file_id).unwrap();
            let mut file = file.borrow_mut();
            if len as usize > file.data.len() {
                file.data.resize(len as usize, 0);
            } else {
                file.data.truncate(len as usize);
            }
            0
        }
        _ => -22,
    }
}

pub fn sys_statfs(path: usize, buf: usize) -> isize {
    let _ = path;
    fill_statfs(buf)
}

pub fn sys_fstatfs(fd: usize, buf: usize) -> isize {
    let _ = fd;
    fill_statfs(buf)
}

fn fill_statfs(buf: usize) -> isize {
    if buf == 0 {
        return -14;
    }
    let mut data = [0u8; 120];
    // f_type = RAMFS magic-ish, f_bsize, f_blocks, f_bfree, f_files, f_ffree
    data[..8].copy_from_slice(&0x858458f6u64.to_le_bytes()); // RAMFS_MAGIC
    data[8..16].copy_from_slice(&4096u64.to_le_bytes()); // f_bsize
    data[24..32].copy_from_slice(&(128 * 1024u64).to_le_bytes()); // f_blocks (128MB)
    data[32..40].copy_from_slice(&(64 * 1024u64).to_le_bytes()); // f_bfree
    data[56..64].copy_from_slice(&1024u64.to_le_bytes()); // f_files
    data[64..72].copy_from_slice(&1024u64.to_le_bytes()); // f_ffree
    match write_user(buf, &data) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_sendfile(out_fd: usize, in_fd: usize, offset_ptr: usize, count: usize) -> isize {
    let f = match get_fd(in_fd) {
        Some(f) => f,
        None => return -9,
    };
    let file_id = match &f.kind {
        FdKind::File { file_id } => *file_id,
        _ => return -22,
    };
    let file = fs::fs().get(file_id).unwrap();
    let data = file.borrow().data.clone();
    let off = if offset_ptr != 0 {
        let d = match read_user(offset_ptr, 8) {
            Ok(d) => d,
            Err(e) => return e as isize,
        };
        u64::from_le_bytes(d[..8].try_into().unwrap()) as usize
    } else {
        f.offset as usize
    };
    if off >= data.len() {
        return 0;
    }
    let n = core::cmp::min(count, data.len() - off);
    let chunk = &data[off..off + n];
    let out = match get_fd(out_fd) {
        Some(o) => o,
        None => return -9,
    };
    match fs::write_fd(out, chunk) {
        Ok(written) => {
            if offset_ptr != 0 {
                let new_off = (off + written) as u64;
                let _ = write_user(offset_ptr, &new_off.to_le_bytes());
            } else {
                f.offset = (off + written) as u64;
            }
            written as isize
        }
        Err(e) => e as isize,
    }
}

pub fn sys_pipe2(fds_ptr: usize, flags: usize) -> isize {
    // create an in-memory pipe pair via unix socketpair
    let (a, b) = match crate::net::sock_socketpair(1) {
        Ok(p) => p,
        Err(e) => return e as isize,
    };
    let fds = unsafe { &mut *cur_fds() };
    let fa = match fds.alloc() {
        Some(f) => f,
        None => return -24,
    };
    let fb = match fds.alloc() {
        Some(f) => f,
        None => return -24,
    };
    let nonblock = flags & fs::O_NONBLOCK as usize != 0;
    fds.fds[fa] = Some(Fd {
        kind: FdKind::UnixPair { sock_id: a },
        flags: 0,
        offset: 0,
        cloexec: flags & fs::O_CLOEXEC as usize != 0,
        epoll: None,
    });
    fds.fds[fb] = Some(Fd {
        kind: FdKind::UnixPair { sock_id: b },
        flags: 0,
        offset: 0,
        cloexec: flags & fs::O_CLOEXEC as usize != 0,
        epoll: None,
    });
    crate::net::sock(a).unwrap().nonblock = nonblock;
    crate::net::sock(b).unwrap().nonblock = nonblock;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(fa as u32).to_le_bytes());
    out[4..].copy_from_slice(&(fb as u32).to_le_bytes());
    match write_user(fds_ptr, &out) {
        Ok(_) => 0,
        Err(e) => e as isize,
    }
}

pub fn sys_eventfd2(init: usize, flags: usize) -> isize {
    let _ = flags;
    let fds = unsafe { &mut *cur_fds() };
    let fdnum = match fds.alloc() {
        Some(f) => f,
        None => return -24,
    };
    fds.fds[fdnum] = Some(Fd {
        kind: FdKind::Eventfd {
            counter: init as u64,
            flags: 0,
        },
        flags: 0,
        offset: 0,
        cloexec: flags & fs::O_CLOEXEC as usize != 0,
        epoll: None,
    });
    fdnum as isize
}

pub fn sys_readlinkat(dirfd: isize, path: usize, buf: usize, bufsize: usize) -> isize {
    let _ = (dirfd, buf, bufsize);
    let path_str = match read_cstr(path, 4096) {
        Ok(s) => s,
        Err(e) => return e as isize,
    };
    let _ = path_str;
    -22 // no symlinks
}
