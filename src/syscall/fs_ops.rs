//! Filesystem syscalls.

use crate::fs::path::{self, AT_EMPTY_PATH, AT_FDCWD, AT_REMOVEDIR, AT_SYMLINK_NOFOLLOW};
use crate::fs::stat::{Dirent64, Kstat, StatFs, S_IFMT};
use crate::fs::{self, File, InodeKind, OpenFlags, Result, SeekFrom};
use crate::mm::uaccess;
use crate::{bail, task};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Fetch a path argument from user space.
fn user_path(ptr: usize) -> Result<alloc::string::String> {
    if ptr == 0 {
        bail!(EFAULT);
    }
    Ok(uaccess::read_cstr(ptr)?)
}

pub fn sys_read(fd: i32, buf: usize, len: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    if len == 0 {
        return Ok(0);
    }
    // Cap a single read so a bogus huge length can't exhaust the heap.
    let len = len.min(16 * 1024 * 1024);
    let mut data = alloc::vec![0u8; len];
    let n = file.read(&mut data)?;
    uaccess::write_bytes(buf, &data[..n])?;
    Ok(n as isize)
}

pub fn sys_write(fd: i32, buf: usize, len: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    if len == 0 {
        return Ok(0);
    }
    let len = len.min(16 * 1024 * 1024);
    let data = uaccess::read_bytes(buf, len)?;
    let n = file.write(&data)?;
    Ok(n as isize)
}

pub fn sys_readv(fd: i32, iov: usize, count: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    let vecs = uaccess::read_iovecs(iov, count)?;
    let mut total = 0isize;
    for v in vecs {
        if v.len == 0 {
            continue;
        }
        let mut data = alloc::vec![0u8; v.len.min(16 * 1024 * 1024)];
        let n = match file.read(&mut data) {
            Ok(n) => n,
            Err(e) => {
                if total > 0 {
                    return Ok(total);
                }
                return Err(e);
            }
        };
        uaccess::write_bytes(v.base, &data[..n])?;
        total += n as isize;
        // A short read ends the operation.
        if n < data.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_writev(fd: i32, iov: usize, count: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    let vecs = uaccess::read_iovecs(iov, count)?;
    let mut total = 0isize;
    for v in vecs {
        if v.len == 0 {
            continue;
        }
        let data = uaccess::read_bytes(v.base, v.len.min(16 * 1024 * 1024))?;
        let n = match file.write(&data) {
            Ok(n) => n,
            Err(e) => {
                // Report a partial write rather than an error if we made progress.
                if total > 0 {
                    return Ok(total);
                }
                return Err(e);
            }
        };
        total += n as isize;
        if n < data.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_pread(fd: i32, buf: usize, len: usize, offset: i64) -> Result<isize> {
    if offset < 0 {
        bail!(EINVAL);
    }
    let file = task::current().files.lock().get_or_err(fd)?;
    let mut data = alloc::vec![0u8; len.min(16 * 1024 * 1024)];
    let n = file.read_at(offset as usize, &mut data)?;
    uaccess::write_bytes(buf, &data[..n])?;
    Ok(n as isize)
}

pub fn sys_pwrite(fd: i32, buf: usize, len: usize, offset: i64) -> Result<isize> {
    if offset < 0 {
        bail!(EINVAL);
    }
    let file = task::current().files.lock().get_or_err(fd)?;
    if !file.writable() {
        bail!(EBADF);
    }
    let data = uaccess::read_bytes(buf, len.min(16 * 1024 * 1024))?;
    let n = file.write_at(offset as usize, &data)?;
    Ok(n as isize)
}

pub fn sys_openat(dirfd: i32, path_ptr: usize, flags: u32, mode: u32) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let flags = OpenFlags::from_bits_truncate(flags);
    let task = task::current();

    let inode = match path::resolve_at(dirfd, &path_str, !flags.contains(OpenFlags::NOFOLLOW)) {
        Ok(inode) => {
            if flags.contains(OpenFlags::CREAT) && flags.contains(OpenFlags::EXCL) {
                bail!(EEXIST);
            }
            inode
        }
        Err(e) if flags.contains(OpenFlags::CREAT) && e.errno() == fs::errno::ENOENT => {
            // Create it.
            let (dir, name) = path::resolve_parent_at(dirfd, &path_str)?;
            let effective_mode = mode & !task.umask();
            dir.create(&name, InodeKind::File, effective_mode)?
        }
        Err(e) => return Err(e),
    };

    if flags.contains(OpenFlags::DIRECTORY) && inode.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    if inode.kind() == InodeKind::Dir && flags.writable() {
        bail!(EISDIR);
    }
    if flags.contains(OpenFlags::TRUNC) && inode.kind() == InodeKind::File && flags.writable() {
        inode.truncate(0)?;
    }

    let file = Arc::new(File::with_path(inode, flags, &path_str));
    if flags.contains(OpenFlags::APPEND) {
        let size = file.inode.size();
        file.set_offset(size);
    }
    let cloexec = flags.contains(OpenFlags::CLOEXEC);
    let fd = task.files.lock().insert(file, cloexec)?;
    Ok(fd as isize)
}

pub fn sys_close(fd: i32) -> Result<isize> {
    task::current().files.lock().close(fd)?;
    Ok(0)
}

pub fn sys_lseek(fd: i32, offset: i64, whence: u32) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    let pos = match whence {
        0 => SeekFrom::Start(offset),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => bail!(EINVAL),
    };
    Ok(file.seek(pos)? as isize)
}

pub fn sys_dup(fd: i32) -> Result<isize> {
    let task = task::current();
    let file = task.files.lock().get_or_err(fd)?;
    let new_fd = task.files.lock().insert(file, false)?;
    Ok(new_fd as isize)
}

pub fn sys_dup3(old: i32, new: i32, flags: u32) -> Result<isize> {
    let task = task::current();
    let file = task.files.lock().get_or_err(old)?;
    if old == new {
        // `dup3` rejects this; `dup2` (which musl implements via dup3) returns
        // the fd, but musl handles that case itself before calling us.
        bail!(EINVAL);
    }
    let cloexec = flags & OpenFlags::CLOEXEC.bits() != 0;
    let fd = task.files.lock().insert_at(new, file, cloexec)?;
    Ok(fd as isize)
}

// `fcntl` commands.
const F_DUPFD: u32 = 0;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const F_SETLK: u32 = 6;
const F_SETLKW: u32 = 7;
const F_GETLK: u32 = 5;
const F_SETOWN: u32 = 8;
const F_GETOWN: u32 = 9;
const F_DUPFD_CLOEXEC: u32 = 1030;
const F_SETPIPE_SZ: u32 = 1031;
const F_GETPIPE_SZ: u32 = 1032;

pub fn sys_fcntl(fd: i32, cmd: u32, arg: usize) -> Result<isize> {
    let task = task::current();
    let file = task.files.lock().get_or_err(fd)?;
    match cmd {
        F_DUPFD => {
            let new_fd = task.files.lock().insert_from(file, arg, false)?;
            Ok(new_fd as isize)
        }
        F_DUPFD_CLOEXEC => {
            let new_fd = task.files.lock().insert_from(file, arg, true)?;
            Ok(new_fd as isize)
        }
        F_GETFD => {
            let cloexec = task.files.lock().get_cloexec(fd)?;
            Ok(if cloexec { 1 } else { 0 })
        }
        F_SETFD => {
            task.files.lock().set_cloexec(fd, arg & 1 != 0)?;
            Ok(0)
        }
        F_GETFL => Ok(file.flags.lock().bits() as isize),
        F_SETFL => {
            // Only these are changeable after open.
            let changeable = OpenFlags::NONBLOCK
                | OpenFlags::APPEND
                | OpenFlags::DIRECT
                | OpenFlags::NOATIME
                | OpenFlags::DSYNC;
            let new = OpenFlags::from_bits_truncate(arg as u32) & changeable;
            let mut flags = file.flags.lock();
            *flags = (*flags & !changeable) | new;
            drop(flags);
            // Sockets keep their own non-blocking flag.
            if let Some(socket) = file.as_socket() {
                socket.nonblock.store(
                    new.contains(OpenFlags::NONBLOCK),
                    core::sync::atomic::Ordering::Relaxed,
                );
            }
            Ok(0)
        }
        // Advisory locks: we run one process per file in practice, and nginx only
        // uses them on its pid file, so granting them unconditionally is safe.
        F_SETLK | F_SETLKW => Ok(0),
        F_GETLK => Ok(0),
        F_SETOWN | F_GETOWN => Ok(0),
        F_SETPIPE_SZ => {
            if let Some(pipe) = file.inode.as_any().downcast_ref::<fs::pipe::PipeEnd>() {
                Ok(fs::pipe::set_capacity(pipe, arg) as isize)
            } else {
                bail!(EINVAL)
            }
        }
        F_GETPIPE_SZ => {
            if let Some(pipe) = file.inode.as_any().downcast_ref::<fs::pipe::PipeEnd>() {
                Ok(fs::pipe::capacity(pipe) as isize)
            } else {
                bail!(EINVAL)
            }
        }
        _ => {
            crate::warn!("fcntl: unsupported command {}", cmd);
            bail!(EINVAL)
        }
    }
}

/// Terminal-independent ioctls handled at the file layer.
const FIONBIO: usize = 0x5421;
const FIONREAD: usize = 0x541b;
const FIOCLEX: usize = 0x5451;
const FIONCLEX: usize = 0x5450;

pub fn sys_ioctl(fd: i32, cmd: usize, arg: usize) -> Result<isize> {
    let task = task::current();
    let file = task.files.lock().get_or_err(fd)?;

    // `FIONBIO` and the cloexec ioctls act on the open file description, not the
    // underlying object, so handle them here for every descriptor kind. nginx
    // sets non-blocking mode this way on its channel sockets.
    match cmd {
        FIONBIO => {
            let on: u32 = uaccess::read(arg)?;
            let mut flags = file.flags.lock();
            if on != 0 {
                *flags |= OpenFlags::NONBLOCK;
            } else {
                *flags &= !OpenFlags::NONBLOCK;
            }
            drop(flags);
            // Sockets track it separately, since blocking decisions happen deep
            // inside the socket code.
            if let Some(socket) = file.as_socket() {
                socket
                    .nonblock
                    .store(on != 0, core::sync::atomic::Ordering::Relaxed);
            }
            return Ok(0);
        }
        FIOCLEX => {
            task.files.lock().set_cloexec(fd, true)?;
            return Ok(0);
        }
        FIONCLEX => {
            task.files.lock().set_cloexec(fd, false)?;
            return Ok(0);
        }
        _ => {}
    }

    match file.inode.ioctl(cmd, arg) {
        // Fall back to a generic FIONREAD for objects that don't implement it.
        Err(e) if cmd == FIONREAD && e.errno() == fs::errno::ENOTTY => {
            let available = file.inode.size().saturating_sub(file.offset());
            uaccess::write(arg, available as u32)?;
            Ok(0)
        }
        other => other,
    }
}

pub fn sys_preadv2(fd: i32, iov: usize, count: usize, offset: i64) -> Result<isize> {
    // A negative offset means "use the file position", i.e. plain `readv`.
    if offset < 0 {
        return sys_readv(fd, iov, count);
    }
    let file = task::current().files.lock().get_or_err(fd)?;
    let vecs = uaccess::read_iovecs(iov, count)?;
    let mut pos = offset as usize;
    let mut total = 0isize;
    for v in vecs {
        if v.len == 0 {
            continue;
        }
        let mut data = alloc::vec![0u8; v.len.min(16 * 1024 * 1024)];
        let n = file.read_at(pos, &mut data)?;
        uaccess::write_bytes(v.base, &data[..n])?;
        pos += n;
        total += n as isize;
        if n < data.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_pwritev2(fd: i32, iov: usize, count: usize, offset: i64) -> Result<isize> {
    if offset < 0 {
        return sys_writev(fd, iov, count);
    }
    let file = task::current().files.lock().get_or_err(fd)?;
    if !file.writable() {
        bail!(EBADF);
    }
    let vecs = uaccess::read_iovecs(iov, count)?;
    let mut pos = offset as usize;
    let mut total = 0isize;
    for v in vecs {
        if v.len == 0 {
            continue;
        }
        let data = uaccess::read_bytes(v.base, v.len.min(16 * 1024 * 1024))?;
        let n = file.write_at(pos, &data)?;
        pos += n;
        total += n as isize;
        if n < data.len() {
            break;
        }
    }
    Ok(total)
}

/// `copy_file_range`.
pub fn sys_copy_file_range(
    fd_in: i32,
    off_in_ptr: usize,
    fd_out: i32,
    off_out_ptr: usize,
    len: usize,
) -> Result<isize> {
    let task = task::current();
    let in_file = task.files.lock().get_or_err(fd_in)?;
    let out_file = task.files.lock().get_or_err(fd_out)?;

    let mut in_off = if off_in_ptr != 0 {
        uaccess::read::<i64>(off_in_ptr)? as usize
    } else {
        in_file.offset()
    };
    let mut out_off = if off_out_ptr != 0 {
        uaccess::read::<i64>(off_out_ptr)? as usize
    } else {
        out_file.offset()
    };

    let mut remaining = len;
    let mut total = 0usize;
    let mut buf = alloc::vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len());
        let n = in_file.read_at(in_off, &mut buf[..want])?;
        if n == 0 {
            break;
        }
        let written = out_file.write_at(out_off, &buf[..n])?;
        in_off += written;
        out_off += written;
        total += written;
        remaining -= written;
        if written < n {
            break;
        }
    }

    if off_in_ptr != 0 {
        uaccess::write(off_in_ptr, in_off as i64)?;
    } else {
        in_file.set_offset(in_off);
    }
    if off_out_ptr != 0 {
        uaccess::write(off_out_ptr, out_off as i64)?;
    } else {
        out_file.set_offset(out_off);
    }
    Ok(total as isize)
}

pub fn sys_fstat(fd: i32, buf: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    let stat = file.inode.stat()?;
    uaccess::write(buf, stat)?;
    Ok(0)
}

pub fn sys_fstatat(dirfd: i32, path_ptr: usize, buf: usize, flags: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;

    let inode = if path_str.is_empty() && flags & AT_EMPTY_PATH != 0 {
        path::at_base(dirfd)?
    } else {
        path::resolve_at(dirfd, &path_str, follow)?
    };
    let stat = inode.stat()?;
    uaccess::write(buf, stat)?;
    Ok(0)
}

/// `struct statx`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Statx {
    stx_mask: u32,
    stx_blksize: u32,
    stx_attributes: u64,
    stx_nlink: u32,
    stx_uid: u32,
    stx_gid: u32,
    stx_mode: u16,
    __spare0: u16,
    stx_ino: u64,
    stx_size: u64,
    stx_blocks: u64,
    stx_attributes_mask: u64,
    stx_atime: StatxTimestamp,
    stx_btime: StatxTimestamp,
    stx_ctime: StatxTimestamp,
    stx_mtime: StatxTimestamp,
    stx_rdev_major: u32,
    stx_rdev_minor: u32,
    stx_dev_major: u32,
    stx_dev_minor: u32,
    stx_mnt_id: u64,
    stx_dio_mem_align: u32,
    stx_dio_offset_align: u32,
    __spare2: [u64; 12],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct StatxTimestamp {
    tv_sec: i64,
    tv_nsec: u32,
    __reserved: i32,
}

pub fn sys_statx(
    dirfd: i32,
    path_ptr: usize,
    flags: usize,
    _mask: u32,
    buf: usize,
) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let inode = if path_str.is_empty() && flags & AT_EMPTY_PATH != 0 {
        path::at_base(dirfd)?
    } else {
        path::resolve_at(dirfd, &path_str, follow)?
    };
    let st = inode.stat()?;

    let mut stx = Statx::default();
    // Report everything we filled in: BASIC_STATS | BTIME.
    stx.stx_mask = 0x7ff | 0x800;
    stx.stx_blksize = st.st_blksize as u32;
    stx.stx_nlink = st.st_nlink;
    stx.stx_uid = st.st_uid;
    stx.stx_gid = st.st_gid;
    stx.stx_mode = st.st_mode as u16;
    stx.stx_ino = st.st_ino;
    stx.stx_size = st.st_size as u64;
    stx.stx_blocks = st.st_blocks as u64;
    for (dst, src) in [
        (&mut stx.stx_atime, st.st_atime),
        (&mut stx.stx_mtime, st.st_mtime),
        (&mut stx.stx_ctime, st.st_ctime),
        (&mut stx.stx_btime, st.st_ctime),
    ] {
        dst.tv_sec = src.sec;
        dst.tv_nsec = src.nsec as u32;
    }
    let (major, minor) = inode.device();
    stx.stx_rdev_major = major;
    stx.stx_rdev_minor = minor;
    stx.stx_dev_major = 0;
    stx.stx_dev_minor = 1;
    uaccess::write(buf, stx)?;
    Ok(0)
}

pub fn sys_getdents64(fd: i32, buf: usize, len: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    if file.inode.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    let entries = file.inode.readdir()?;
    let mut pos = file.dir_pos();
    let mut out: Vec<u8> = Vec::new();

    while pos < entries.len() {
        let entry = &entries[pos];
        let name = entry.name.as_bytes();
        // The record is the header plus the NUL-terminated name, rounded up to
        // 8 bytes so the next record stays aligned.
        let reclen = (core::mem::size_of::<Dirent64>() + name.len() + 1 + 7) & !7;
        if out.len() + reclen > len {
            if out.is_empty() {
                // Not even one entry fits.
                bail!(EINVAL);
            }
            break;
        }
        let header = Dirent64 {
            d_ino: entry.ino,
            d_off: (pos + 1) as i64,
            d_reclen: reclen as u16,
            d_type: entry.kind.dirent_type(),
        };
        let start = out.len();
        out.resize(start + reclen, 0);
        unsafe {
            core::ptr::copy_nonoverlapping(
                &header as *const _ as *const u8,
                out[start..].as_mut_ptr(),
                core::mem::size_of::<Dirent64>(),
            );
        }
        let name_off = start + core::mem::size_of::<Dirent64>();
        out[name_off..name_off + name.len()].copy_from_slice(name);
        pos += 1;
    }

    file.set_dir_pos(pos);
    if out.is_empty() {
        return Ok(0);
    }
    uaccess::write_bytes(buf, &out)?;
    Ok(out.len() as isize)
}

pub fn sys_mkdirat(dirfd: i32, path_ptr: usize, mode: u32) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let (dir, name) = path::resolve_parent_at(dirfd, &path_str)?;
    let mode = mode & !task::current().umask();
    dir.create(&name, InodeKind::Dir, mode)?;
    Ok(0)
}

pub fn sys_mknodat(dirfd: i32, path_ptr: usize, mode: u32, _dev: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let (dir, name) = path::resolve_parent_at(dirfd, &path_str)?;
    let kind = match mode & S_IFMT {
        0o010000 => InodeKind::Fifo,
        0o140000 => InodeKind::Socket,
        0 | 0o100000 => InodeKind::File,
        _ => bail!(EPERM),
    };
    dir.create(&name, kind, mode & 0o7777)?;
    Ok(0)
}

pub fn sys_unlinkat(dirfd: i32, path_ptr: usize, flags: u32) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let (dir, name) = path::resolve_parent_at(dirfd, &path_str)?;
    let target = dir.lookup(&name)?;
    let want_dir = flags as usize & AT_REMOVEDIR != 0;
    if want_dir && target.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    if !want_dir && target.kind() == InodeKind::Dir {
        bail!(EISDIR);
    }
    dir.unlink(&name)?;
    Ok(0)
}

pub fn sys_symlinkat(target_ptr: usize, dirfd: i32, path_ptr: usize) -> Result<isize> {
    let target = user_path(target_ptr)?;
    let path_str = user_path(path_ptr)?;
    let (dir, name) = path::resolve_parent_at(dirfd, &path_str)?;
    dir.symlink(&name, &target)?;
    Ok(0)
}

pub fn sys_linkat(
    old_dirfd: i32,
    old_ptr: usize,
    new_dirfd: i32,
    new_ptr: usize,
    _flags: u32,
) -> Result<isize> {
    let old = user_path(old_ptr)?;
    let new = user_path(new_ptr)?;
    let inode = path::resolve_at(old_dirfd, &old, true)?;
    if inode.kind() == InodeKind::Dir {
        bail!(EPERM);
    }
    let (dir, name) = path::resolve_parent_at(new_dirfd, &new)?;
    dir.link(&name, &inode)?;
    Ok(0)
}

pub fn sys_renameat(old_dirfd: i32, old_ptr: usize, new_dirfd: i32, new_ptr: usize) -> Result<isize> {
    let old = user_path(old_ptr)?;
    let new = user_path(new_ptr)?;
    let (old_dir, old_name) = path::resolve_parent_at(old_dirfd, &old)?;
    let (new_dir, new_name) = path::resolve_parent_at(new_dirfd, &new)?;
    // Replacing an existing destination is allowed.
    let _ = new_dir.unlink(&new_name);
    old_dir.rename(&old_name, &new_dir, &new_name)?;
    Ok(0)
}

pub fn sys_readlinkat(dirfd: i32, path_ptr: usize, buf: usize, len: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let inode = path::resolve_at(dirfd, &path_str, false)?;
    if inode.kind() != InodeKind::Symlink {
        bail!(EINVAL);
    }
    let target = inode.readlink()?;
    let bytes = target.as_bytes();
    // `readlink` does not NUL-terminate and truncates silently.
    let n = bytes.len().min(len);
    uaccess::write_bytes(buf, &bytes[..n])?;
    Ok(n as isize)
}

pub fn sys_faccessat(dirfd: i32, path_ptr: usize, _mode: u32) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    // We run everything as a privileged user against a ramfs, so existence is
    // the only check that can fail.
    path::resolve_at(dirfd, &path_str, true)?;
    Ok(0)
}

pub fn sys_truncate(path_ptr: usize, len: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let inode = path::resolve(&path_str, true)?;
    inode.truncate(len)?;
    Ok(0)
}

pub fn sys_ftruncate(fd: i32, len: usize) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    if !file.writable() {
        bail!(EINVAL);
    }
    file.inode.truncate(len)?;
    Ok(0)
}

pub fn sys_getcwd(buf: usize, len: usize) -> Result<isize> {
    let cwd = task::current_cwd();
    let p = path::abs_path(&cwd).unwrap_or_else(|| alloc::string::String::from("/"));
    let mut bytes = p.into_bytes();
    bytes.push(0);
    if bytes.len() > len {
        bail!(ERANGE);
    }
    uaccess::write_bytes(buf, &bytes)?;
    // Linux returns the length including the NUL.
    Ok(bytes.len() as isize)
}

pub fn sys_chdir(path_ptr: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let inode = path::resolve(&path_str, true)?;
    if inode.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    *task::current().cwd.lock() = inode;
    Ok(0)
}

pub fn sys_fchdir(fd: i32) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    if file.inode.kind() != InodeKind::Dir {
        bail!(ENOTDIR);
    }
    *task::current().cwd.lock() = file.inode.clone();
    Ok(0)
}

pub fn sys_fchmod(fd: i32, mode: u32) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    file.inode.set_mode(mode);
    Ok(0)
}

pub fn sys_fchmodat(dirfd: i32, path_ptr: usize, mode: u32) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let inode = path::resolve_at(dirfd, &path_str, true)?;
    inode.set_mode(mode);
    Ok(0)
}

pub fn sys_fchown(fd: i32, uid: u32, gid: u32) -> Result<isize> {
    let file = task::current().files.lock().get_or_err(fd)?;
    file.inode.set_owner(uid, gid);
    Ok(0)
}

pub fn sys_fchownat(
    dirfd: i32,
    path_ptr: usize,
    uid: u32,
    gid: u32,
    flags: usize,
) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    let inode = path::resolve_at(dirfd, &path_str, flags & AT_SYMLINK_NOFOLLOW == 0)?;
    inode.set_owner(uid, gid);
    Ok(0)
}

pub fn sys_pipe2(fds_ptr: usize, flags: u32) -> Result<isize> {
    let (read_end, write_end) = fs::pipe::new_pipe();
    let mut open_flags = OpenFlags::empty();
    if flags & OpenFlags::NONBLOCK.bits() != 0 {
        open_flags |= OpenFlags::NONBLOCK;
    }
    let cloexec = flags & OpenFlags::CLOEXEC.bits() != 0;

    let read_file = Arc::new(File::with_path(
        read_end,
        open_flags | OpenFlags::RDONLY,
        "pipe:[read]",
    ));
    let write_file = Arc::new(File::with_path(
        write_end,
        open_flags | OpenFlags::WRONLY,
        "pipe:[write]",
    ));

    let task = task::current();
    let read_fd = task.files.lock().insert(read_file, cloexec)?;
    let write_fd = match task.files.lock().insert(write_file, cloexec) {
        Ok(fd) => fd,
        Err(e) => {
            // Don't leak the read end if we ran out of descriptors.
            let _ = task.files.lock().close(read_fd);
            return Err(e);
        }
    };
    uaccess::write(fds_ptr, [read_fd, write_fd])?;
    Ok(0)
}

pub fn sys_sendfile(out_fd: i32, in_fd: i32, offset_ptr: usize, count: usize) -> Result<isize> {
    let task = task::current();
    let out_file = task.files.lock().get_or_err(out_fd)?;
    let in_file = task.files.lock().get_or_err(in_fd)?;
    if !out_file.writable() || !in_file.readable() {
        bail!(EBADF);
    }

    // If `offset_ptr` is given, read from there and update it without touching
    // the file's cursor.
    let explicit_offset = if offset_ptr != 0 {
        Some(uaccess::read::<i64>(offset_ptr)? as usize)
    } else {
        None
    };
    let mut offset = explicit_offset.unwrap_or_else(|| in_file.offset());

    // Copy in chunks so a huge `count` doesn't need one huge buffer.
    const CHUNK: usize = 64 * 1024;
    let mut remaining = count;
    let mut total = 0usize;
    let mut buf = alloc::vec![0u8; CHUNK.min(count.max(1))];

    while remaining > 0 {
        let want = remaining.min(buf.len());
        let n = in_file.read_at(offset, &mut buf[..want])?;
        if n == 0 {
            break; // EOF
        }
        let mut written = 0;
        while written < n {
            match out_file.write(&buf[written..n]) {
                Ok(0) => break,
                Ok(w) => written += w,
                Err(e) => {
                    // Report progress if we made any, as Linux does.
                    if total + written > 0 {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        offset += written;
        total += written;
        remaining -= written;
        if written < n {
            break; // the output blocked or closed
        }
    }

    match explicit_offset {
        Some(_) => uaccess::write(offset_ptr, offset as i64)?,
        None => in_file.set_offset(offset),
    }
    Ok(total as isize)
}

fn make_statfs() -> StatFs {
    let (used, total) = crate::mm::frame::stats();
    StatFs {
        // TMPFS_MAGIC, since our root behaves like tmpfs.
        f_type: 0x0102_9994,
        f_bsize: 4096,
        f_blocks: total as u64,
        f_bfree: (total - used) as u64,
        f_bavail: (total - used) as u64,
        f_files: 1 << 20,
        f_ffree: 1 << 19,
        f_fsid: [0, 0],
        f_namelen: 255,
        f_frsize: 4096,
        f_flags: 0,
        f_spare: [0; 4],
    }
}

pub fn sys_statfs(path_ptr: usize, buf: usize) -> Result<isize> {
    let path_str = user_path(path_ptr)?;
    // Verify the path exists, then report the same figures for every mount.
    path::resolve(&path_str, true)?;
    uaccess::write(buf, make_statfs())?;
    Ok(0)
}

pub fn sys_fstatfs(fd: i32, buf: usize) -> Result<isize> {
    task::current().files.lock().get_or_err(fd)?;
    uaccess::write(buf, make_statfs())?;
    Ok(0)
}

pub fn sys_memfd_create(name_ptr: usize, flags: u32) -> Result<isize> {
    let name = if name_ptr != 0 {
        uaccess::read_cstr(name_ptr)?
    } else {
        alloc::string::String::from("memfd")
    };
    let inode = fs::ramfs::RamFile::new(0o600);
    let file = Arc::new(File::with_path(
        inode,
        OpenFlags::RDWR,
        &alloc::format!("/memfd:{}", name),
    ));
    // MFD_CLOEXEC is bit 0.
    let cloexec = flags & 1 != 0;
    let fd = task::current().files.lock().insert(file, cloexec)?;
    Ok(fd as isize)
}

pub fn sys_close_range(first: u32, last: u32, _flags: u32) -> Result<isize> {
    task::current().files.lock().close_range(first, last);
    Ok(0)
}

/// Suppress the unused-import warning for `AT_FDCWD`, which documents the
/// convention the `*at` helpers rely on.
const _: i32 = AT_FDCWD;
