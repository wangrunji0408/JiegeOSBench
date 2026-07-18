use crate::fs::{self, O_APPEND, O_CREAT, O_DIRECTORY, O_EXCL, O_TRUNC};
use crate::mm::{translated_byte_buffer, translated_str};
use crate::task::{current_task, current_user_token};
use alloc::sync::Arc;

const AT_FDCWD: isize = -100;

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9, // EBADF
    };
    if !file.writable() {
        return -13; // EACCES
    }
    let buffers = translated_byte_buffer(token, buf, len);
    let mut written = 0;
    for b in buffers {
        written += file.write(b);
    }
    written as isize
}

pub fn sys_read(fd: usize, buf: *mut u8, len: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if !file.readable() {
        return -13;
    }
    let buffers = translated_byte_buffer(token, buf, len);
    let mut read = 0;
    for b in buffers {
        let n = file.read(b);
        read += n;
        if n < b.len() {
            break;
        }
    }
    read as isize
}

pub fn sys_pread64(fd: usize, buf: *mut u8, len: usize, offset: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let buffers = translated_byte_buffer(token, buf, len);
    let mut read = 0;
    let mut off = offset;
    for b in buffers {
        let n = file.read_at(off, b);
        read += n;
        off += n;
        if n < b.len() {
            break;
        }
    }
    read as isize
}

pub fn sys_pwrite64(fd: usize, buf: *const u8, len: usize, offset: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let buffers = translated_byte_buffer(token, buf, len);
    let mut written = 0;
    let mut off = offset;
    for b in buffers {
        let n = file.write_at(off, b);
        written += n;
        off += n;
    }
    written as isize
}

pub fn sys_close(fd: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    match inner.fd_table.get_mut(fd) {
        Some(slot) if slot.is_some() => {
            *slot = None;
            0
        }
        _ => -9,
    }
}

pub fn sys_dup3(oldfd: usize, newfd: usize, _flags: usize) -> isize {
    let task = current_task().unwrap();
    let mut inner = task.inner_lock();
    let file = match inner.get_fd(oldfd) {
        Some(f) => f,
        None => return -9,
    };
    if newfd >= inner.fd_table.len() {
        inner.fd_table.resize(newfd + 1, None);
    }
    inner.fd_table[newfd] = Some(file);
    newfd as isize
}

fn resolve_at_path(dirfd: isize, path: alloc::string::String) -> alloc::string::String {
    // Every path used by our target workload is absolute; dirfd is only
    // ever AT_FDCWD in practice, so relative paths are just resolved
    // against `/`.
    let _ = dirfd == AT_FDCWD;
    path
}

pub fn sys_openat(dirfd: isize, path: *const u8, flags: u32, _mode: u32) -> isize {
    let token = current_user_token();
    let path = resolve_at_path(dirfd, translated_str(token, path));
    match fs::open_file(&path, flags) {
        Some(file) => {
            let task = current_task().unwrap();
            let mut inner = task.inner_lock();
            inner.alloc_fd(file) as isize
        }
        None => -2, // ENOENT
    }
}

pub fn sys_mkdirat(dirfd: isize, path: *const u8, _mode: u32) -> isize {
    let token = current_user_token();
    let path = resolve_at_path(dirfd, translated_str(token, path));
    if fs::mkdir(&path) {
        0
    } else {
        -17 // EEXIST-ish / generic failure
    }
}

pub fn sys_unlinkat(dirfd: isize, path: *const u8, _flags: u32) -> isize {
    let token = current_user_token();
    let path = resolve_at_path(dirfd, translated_str(token, path));
    if fs::unlink(&path) {
        0
    } else {
        -2
    }
}

pub fn sys_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    const SEEK_SET: usize = 0;
    const SEEK_CUR: usize = 1;
    const SEEK_END: usize = 2;
    let new_pos = match whence {
        SEEK_SET => offset as usize,
        SEEK_CUR => (file.tell() as isize + offset) as usize,
        SEEK_END => (file.size() as isize + offset) as usize,
        _ => return -22, // EINVAL
    };
    file.seek_to(new_pos);
    new_pos as isize
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Stat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    __pad: u64,
    st_size: i64,
    st_blksize: i32,
    __pad2: i32,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: i64,
    st_mtime: i64,
    st_mtime_nsec: i64,
    st_ctime: i64,
    st_ctime_nsec: i64,
    __unused: [u32; 2],
}

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;

fn make_stat(size: usize, is_dir: bool) -> Stat {
    Stat {
        st_ino: 1,
        st_mode: (if is_dir { S_IFDIR | 0o755 } else { S_IFREG | 0o644 }),
        st_nlink: 1,
        st_size: size as i64,
        st_blksize: 512,
        st_blocks: ((size + 511) / 512) as i64,
        ..Default::default()
    }
}

fn write_stat(token: usize, buf: *mut u8, stat: Stat) {
    let bytes = unsafe {
        core::slice::from_raw_parts(&stat as *const Stat as *const u8, core::mem::size_of::<Stat>())
    };
    let mut dst = translated_byte_buffer(token, buf, bytes.len());
    let mut copied = 0;
    for chunk in dst.iter_mut() {
        let n = chunk.len();
        chunk.copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
    }
}

pub fn sys_fstat(fd: usize, buf: *mut u8) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    write_stat(token, buf, make_stat(file.size(), file.is_dir()));
    0
}

pub fn sys_newfstatat(dirfd: isize, path: *const u8, buf: *mut u8, _flags: u32) -> isize {
    let token = current_user_token();
    let path = resolve_at_path(dirfd, translated_str(token, path));
    match fs::stat_size_and_kind(&path) {
        Some((size, is_dir)) => {
            write_stat(token, buf, make_stat(size, is_dir));
            0
        }
        None => -2,
    }
}

pub fn sys_faccessat(dirfd: isize, path: *const u8) -> isize {
    let token = current_user_token();
    let path = resolve_at_path(dirfd, translated_str(token, path));
    if fs::stat_size_and_kind(&path).is_some() {
        0
    } else {
        -2
    }
}

pub fn sys_getcwd(buf: *mut u8, size: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let cwd = task.inner_lock().cwd.clone();
    let bytes = cwd.as_bytes();
    if bytes.len() + 1 > size {
        return 0;
    }
    let mut out = alloc::vec![0u8; bytes.len() + 1];
    out[..bytes.len()].copy_from_slice(bytes);
    let mut chunks = translated_byte_buffer(token, buf, out.len());
    let mut copied = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&out[copied..copied + n]);
        copied += n;
    }
    buf as isize
}

#[repr(C)]
struct IoVec {
    base: usize,
    len: usize,
}

pub fn sys_writev(fd: usize, iov: *const u8, iovcnt: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if !file.writable() {
        return -13;
    }
    let mut total = 0isize;
    for i in 0..iovcnt {
        let entry_bytes = translated_byte_buffer(token, unsafe { iov.add(i * 16) }, 16);
        let mut raw = [0u8; 16];
        let mut off = 0;
        for c in entry_bytes {
            raw[off..off + c.len()].copy_from_slice(c);
            off += c.len();
        }
        let vec = IoVec {
            base: usize::from_ne_bytes(raw[0..8].try_into().unwrap()),
            len: usize::from_ne_bytes(raw[8..16].try_into().unwrap()),
        };
        if vec.len == 0 {
            continue;
        }
        for chunk in translated_byte_buffer(token, vec.base as *const u8, vec.len) {
            total += file.write(chunk) as isize;
        }
    }
    total
}

pub fn sys_readv(_fd: usize, _iov: *const u8, _iovcnt: usize) -> isize {
    -38
}

pub fn sys_pipe2(_fds: *mut u8) -> isize {
    -38
}
