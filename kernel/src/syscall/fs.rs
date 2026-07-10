/// 文件系统相关syscall

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::task::{current_task, manager::TASK_MANAGER};
use crate::task::process::FileDesc;
use crate::fs::{FS, FileStat, FileType};
use crate::fs::ramfs::{INode, INodeKind};

use super::*;

// O_* flags
const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_RDWR: i32 = 2;
const O_CREAT: i32 = 0o100;
const O_TRUNC: i32 = 0o1000;
const O_APPEND: i32 = 0o2000;
const O_NONBLOCK: i32 = 0o4000;
const O_DIRECTORY: i32 = 0o200000;
const O_CLOEXEC: i32 = 0o2000000;
const O_NOFOLLOW: i32 = 0o400000;
const O_LARGEFILE: i32 = 0o100000;
const O_PATH: i32 = 0o10000000;
const O_TMPFILE: i32 = 0o20200000;

const AT_FDCWD: i32 = -100;
const AT_EMPTY_PATH: i32 = 0x1000;
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;

fn get_path(dirfd: i32, path_va: usize) -> Option<String> {
    let task = current_task()?;
    let task = task.lock();
    let path = task.memory_set.page_table.read_cstr(path_va);

    if path.starts_with('/') {
        return Some(path);
    }

    let base = if dirfd == AT_FDCWD {
        task.cwd.clone()
    } else {
        // 获取dirfd对应的路径
        match task.fds.get(&dirfd)? {
            FileDesc::File { .. } => return None, // 不是目录
            _ => task.cwd.clone(),
        }
    };

    if base.ends_with('/') {
        Some(format!("{}{}", base, path))
    } else {
        Some(format!("{}/{}", base, path))
    }
}

fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn sys_openat(dirfd: i32, path_va: usize, flags: i32, mode: u32) -> isize {
    let path = match get_path(dirfd, path_va) {
        Some(p) => normalize_path(&p),
        None => return ENOENT,
    };

    // 检查O_CREAT
    if flags & O_CREAT != 0 {
        if FS.lookup(&path).is_none() {
            FS.create_file(&path, Vec::new(), mode & 0o7777);
        }
    }

    let node = match FS.lookup(&path) {
        Some(n) => n,
        None => return ENOENT,
    };

    // 检查是否是目录
    {
        let n = node.lock();
        if let INodeKind::Dir(_) = &n.kind {
            if flags & O_DIRECTORY == 0 && flags & O_PATH == 0 {
                // 用O_RDONLY打开目录也是OK的（nginx会这么做）
                // return EISDIR;
            }
        }
    }

    // 截断
    if flags & O_TRUNC != 0 {
        let n = node.lock();
        if let INodeKind::File(data) = &n.kind {
            data.lock().clear();
        }
    }

    let task = current_task().unwrap();
    let mut task = task.lock();
    let fd = task.alloc_fd();

    let offset = if flags & O_APPEND != 0 {
        let n = node.lock();
        if let INodeKind::File(data) = &n.kind {
            data.lock().len()
        } else { 0 }
    } else { 0 };

    task.fds.insert(fd, FileDesc::File {
        inode: node,
        offset,
        flags,
    });

    fd as isize
}

pub fn sys_close(fd: i32) -> isize {
    let task = current_task().unwrap();
    let mut task = task.lock();
    match task.fds.remove(&fd) {
        Some(_) => 0,
        None => EBADF,
    }
}

pub fn sys_read(fd: i32, buf_va: usize, count: usize) -> isize {
    if count == 0 { return 0; }

    let task = current_task().unwrap();
    let fd_info = {
        let t = task.lock();
        match t.fds.get(&fd) {
            Some(f) => match f {
                FileDesc::Stdin => return read_stdin(buf_va, count, &t),
                FileDesc::File { inode, offset, flags } => {
                    Some((inode.clone(), *offset, *flags))
                }
                FileDesc::Pipe { read_end, buf } => {
                    if *read_end {
                        return read_pipe(buf.clone(), buf_va, count, &t);
                    }
                    return EBADF;
                }
                FileDesc::Socket(_) => {
                    // Delegate socket reads to recvfrom
                    drop(t);
                    return crate::syscall::net::sys_recvfrom(fd, buf_va, count, 0, 0, 0);
                }
                _ => None,
            },
            None => return EBADF,
        }
    };

    let Some((inode, offset, _flags)) = fd_info else {
        return EBADF;
    };

    let node = inode.lock();
    let data = match &node.kind {
        INodeKind::File(data) => data.lock(),
        INodeKind::CharDev { major: 1, minor: 3 } => return 0, // /dev/null
        INodeKind::CharDev { major: 1, minor: 5 } => {
            // /dev/zero
            let buf = vec![0u8; count];
            task.lock().memory_set.copy_to_user(buf_va, &buf);
            return count as isize;
        }
        INodeKind::CharDev { major: 1, minor: 8 | 9 } => {
            // /dev/random, /dev/urandom - 返回伪随机数
            let mut buf = vec![0u8; count];
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (crate::timer::get_time_us() as u8).wrapping_add(i as u8);
            }
            task.lock().memory_set.copy_to_user(buf_va, &buf);
            return count as isize;
        }
        _ => return EINVAL,
    };

    let available = data.len().saturating_sub(offset);
    if available == 0 { return 0; }

    let n = count.min(available);
    task.lock().memory_set.copy_to_user(buf_va, &data[offset..offset + n]);

    // 更新offset
    {
        let mut t = task.lock();
        if let Some(FileDesc::File { offset: off, .. }) = t.fds.get_mut(&fd) {
            *off += n;
        }
    }

    n as isize
}

fn read_stdin(_buf_va: usize, _count: usize, _task: &crate::task::process::Task) -> isize {
    // 内核中没有终端输入，返回EOF
    0
}

fn read_pipe(pipe: Arc<Mutex<Vec<u8>>>, buf_va: usize, count: usize, task: &crate::task::process::Task) -> isize {
    let data = pipe.lock();
    if data.is_empty() {
        return EAGAIN;
    }
    let n = count.min(data.len());
    task.memory_set.copy_to_user(buf_va, &data[..n]);
    n as isize
}

pub fn sys_write(fd: i32, buf_va: usize, count: usize) -> isize {
    if count == 0 { return 0; }

    let task = current_task().unwrap();
    let t = task.lock();

    let mut buf = vec![0u8; count];
    t.memory_set.copy_from_user(buf_va, &mut buf);

    match t.fds.get(&fd) {
        Some(FileDesc::Stdout) | Some(FileDesc::Stderr) => {
            // 输出到控制台
            if let Ok(s) = core::str::from_utf8(&buf) {
                print!("{}", s);
            }
            count as isize
        }
        Some(FileDesc::File { inode, offset: _, flags }) => {
            let node = inode.lock();
            match &node.kind {
                INodeKind::File(data) => {
                    let mut data = data.lock();
                    let offset = match t.fds.get(&fd) {
                        Some(FileDesc::File { offset, .. }) => *offset,
                        _ => return EBADF,
                    };
                    // 扩展文件（如果需要）
                    if offset > data.len() {
                        data.resize(offset, 0);
                    }
                    if offset == data.len() {
                        data.extend_from_slice(&buf);
                    } else {
                        let end = offset + count;
                        if end > data.len() {
                            data.resize(end, 0);
                        }
                        data[offset..end].copy_from_slice(&buf);
                    }
                    drop(data);
                    drop(node);
                    drop(t);
                    // 更新offset
                    let mut t = task.lock();
                    if let Some(FileDesc::File { offset: off, .. }) = t.fds.get_mut(&fd) {
                        *off += count;
                    }
                    count as isize
                }
                INodeKind::CharDev { major: 1, minor: 3 } => count as isize, // /dev/null
                _ => count as isize, // 忽略其他设备写入
            }
        }
        Some(FileDesc::Pipe { read_end: false, buf: pbuf }) => {
            pbuf.lock().extend_from_slice(&buf);
            count as isize
        }
        Some(FileDesc::Socket(_)) => {
            // Delegate socket writes to sendto
            drop(t);
            crate::syscall::net::sys_sendto(fd, buf_va, count, 0, 0, 0)
        }
        None => EBADF,
        _ => EBADF,
    }
}

pub fn sys_readv(fd: i32, iov_va: usize, iovcnt: i32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    // struct iovec { iov_base: *mut void, iov_len: size_t }
    let mut total = 0isize;
    for i in 0..iovcnt as usize {
        let iov_ptr = iov_va + i * 16;
        let mut iov = [0u8; 16];
        t.memory_set.copy_from_user(iov_ptr, &mut iov);
        let base = usize::from_le_bytes(iov[0..8].try_into().unwrap());
        let len = usize::from_le_bytes(iov[8..16].try_into().unwrap());
        drop(t);
        let n = sys_read(fd, base, len);
        if n < 0 { return n; }
        total += n;
        let t2 = task.lock();
        // 重新借用（为了下次循环）
        // 这里有个借用问题，简化处理
        break; // TODO: 正确处理多个iov
    }
    total
}

pub fn sys_writev(fd: i32, iov_va: usize, iovcnt: i32) -> isize {
    let task = current_task().unwrap();
    let mut total = 0isize;

    for i in 0..iovcnt as usize {
        let iov_ptr = iov_va + i * 16;
        let mut iov = [0u8; 16];
        {
            let t = task.lock();
            t.memory_set.copy_from_user(iov_ptr, &mut iov);
        }
        let base = usize::from_le_bytes(iov[0..8].try_into().unwrap());
        let len = usize::from_le_bytes(iov[8..16].try_into().unwrap());
        if fd <= 2 && len > 0 && len < 4096 {
            // 打印writev到stderr/stdout的内容
            let t = task.lock();
            let mut buf = vec![0u8; len];
            t.memory_set.copy_from_user(base, &mut buf);
            if let Ok(s) = core::str::from_utf8(&buf) {
                if !s.is_empty() {
                    print!("[nginx] {}", s);
                }
            }
            drop(t);
        }
        let n = sys_write(fd, base, len);
        if n < 0 { return n; }
        total += n;
    }
    total
}

pub fn sys_pread64(fd: i32, buf_va: usize, count: usize, offset: i64) -> isize {
    // 先保存当前offset，修改，读取，然后恢复
    let task = current_task().unwrap();
    let old_offset = {
        let t = task.lock();
        match t.fds.get(&fd) {
            Some(FileDesc::File { offset, .. }) => *offset,
            _ => return EBADF,
        }
    };

    {
        let mut t = task.lock();
        if let Some(FileDesc::File { offset: off, .. }) = t.fds.get_mut(&fd) {
            *off = offset as usize;
        }
    }

    let n = sys_read(fd, buf_va, count);

    {
        let mut t = task.lock();
        if let Some(FileDesc::File { offset: off, .. }) = t.fds.get_mut(&fd) {
            *off = old_offset;
        }
    }

    n
}

pub fn sys_lseek(fd: i32, offset: i64, whence: i32) -> isize {
    const SEEK_SET: i32 = 0;
    const SEEK_CUR: i32 = 1;
    const SEEK_END: i32 = 2;

    let task = current_task().unwrap();
    let mut t = task.lock();

    match t.fds.get_mut(&fd) {
        Some(FileDesc::File { inode, offset: cur_off, .. }) => {
            let file_size = {
                let node = inode.lock();
                match &node.kind {
                    INodeKind::File(data) => data.lock().len(),
                    _ => 0,
                }
            };
            let new_off = match whence {
                SEEK_SET => offset as usize,
                SEEK_CUR => (*cur_off as i64 + offset) as usize,
                SEEK_END => (file_size as i64 + offset) as usize,
                _ => return EINVAL,
            };
            *cur_off = new_off;
            new_off as isize
        }
        None => EBADF,
        _ => ESPIPE,
    }
}

pub fn sys_ioctl(fd: i32, cmd: usize, arg: usize) -> isize {
    // TIOCGWINSZ, FIONREAD等
    const TIOCGWINSZ: usize = 0x5413;
    const FIONREAD: usize = 0x541B;
    const FIONBIO: usize = 0x5421;
    const TCGETS: usize = 0x5401;

    match cmd {
        TIOCGWINSZ => {
            // 返回窗口大小
            if let Some(task) = current_task() {
                let t = task.lock();
                let data = [0u16; 4]; // rows, cols, xpixel, ypixel
                t.memory_set.copy_to_user(arg, bytemuck_cast(&data));
            }
            0
        }
        FIONREAD => 0,
        FIONBIO => 0,
        TCGETS => super::ENOSYS,
        _ => 0,
    }
}

fn bytemuck_cast<T>(s: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(
            s.as_ptr() as *const u8,
            s.len() * core::mem::size_of::<T>(),
        )
    }
}

pub fn sys_fcntl(fd: i32, cmd: i32, arg: usize) -> isize {
    const F_DUPFD: i32 = 0;
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const F_SETOWN: i32 = 8;   // Set file owner (process/group to receive signals)
    const F_GETOWN: i32 = 9;   // Get file owner
    const F_SETSIG: i32 = 10;  // Set signal for async I/O
    const F_GETSIG: i32 = 11;  // Get signal
    const F_DUPFD_CLOEXEC: i32 = 1030;
    const F_SETLK: i32 = 6;
    const F_SETLKW: i32 = 7;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => sys_dup(fd),
        F_GETFD => 0,
        F_SETFD => 0,
        F_SETOWN | F_SETSIG => 0, // Ignore - used for async IO signals
        F_GETOWN => 0,
        F_GETSIG => 0,
        F_GETFL => {
            // 返回文件标志
            let task = current_task().unwrap();
            let t = task.lock();
            match t.fds.get(&fd) {
                Some(FileDesc::File { flags, .. }) => *flags as isize,
                Some(FileDesc::Stdin) | Some(FileDesc::Stdout) | Some(FileDesc::Stderr) => 0,
                _ => EBADF,
            }
        }
        F_SETFL => {
            // 设置文件标志
            let task = current_task().unwrap();
            let mut t = task.lock();
            match t.fds.get_mut(&fd) {
                Some(FileDesc::File { flags, .. }) => {
                    *flags = arg as i32;
                    0
                }
                _ => 0,
            }
        }
        F_SETLK | F_SETLKW => 0, // 忽略文件锁
        _ => 0, // Return 0 for unknown commands (better than EINVAL)
    }
}

#[repr(C)]
struct Stat {
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
    st_atime_sec: i64,
    st_atime_nsec: i64,
    st_mtime_sec: i64,
    st_mtime_nsec: i64,
    st_ctime_sec: i64,
    st_ctime_nsec: i64,
}

fn fill_stat(stat: &FileStat, dst: &mut Stat) {
    let mode = match stat.file_type {
        FileType::Regular => 0o100000u32,
        FileType::Directory => 0o040000u32,
        FileType::Symlink => 0o120000u32,
        FileType::CharDevice => 0o020000u32,
        FileType::BlockDevice => 0o060000u32,
        FileType::Fifo => 0o010000u32,
        FileType::Socket => 0o140000u32,
    } | stat.mode;

    dst.st_dev = 1;
    dst.st_ino = stat.ino;
    dst.st_mode = mode;
    dst.st_nlink = stat.nlink;
    dst.st_uid = stat.uid;
    dst.st_gid = stat.gid;
    dst.st_rdev = stat.rdev;
    dst.st_size = stat.size as i64;
    dst.st_blksize = 4096;
    dst.st_blocks = (stat.size as i64 + 511) / 512;
    dst.st_atime_sec = 0;
    dst.st_atime_nsec = 0;
    dst.st_mtime_sec = 0;
    dst.st_mtime_nsec = 0;
    dst.st_ctime_sec = 0;
    dst.st_ctime_nsec = 0;
}

pub fn sys_fstat(fd: i32, stat_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let node = match t.fds.get(&fd) {
        Some(FileDesc::File { inode, .. }) => inode.clone(),
        Some(FileDesc::Stdin) | Some(FileDesc::Stdout) | Some(FileDesc::Stderr) => {
            // 伪终端stat
            let mut stat = Stat {
                st_dev: 1, st_ino: 1, st_mode: 0o020666,
                st_nlink: 1, st_uid: 0, st_gid: 0, st_rdev: 0x500,
                _pad1: 0, st_size: 0, st_blksize: 4096, _pad2: 0, st_blocks: 0,
                st_atime_sec: 0, st_atime_nsec: 0,
                st_mtime_sec: 0, st_mtime_nsec: 0,
                st_ctime_sec: 0, st_ctime_nsec: 0,
            };
            t.memory_set.copy_to_user(stat_va, bytemuck_cast(core::slice::from_ref(&stat)));
            return 0;
        }
        _ => return EBADF,
    };

    let node_guard = node.lock();
    let file_stat = node_guard.stat();
    let mut stat: Stat = unsafe { core::mem::zeroed() };
    fill_stat(&file_stat, &mut stat);
    drop(node_guard);
    t.memory_set.copy_to_user(stat_va, bytemuck_cast(core::slice::from_ref(&stat)));
    0
}

pub fn sys_newfstatat(dirfd: i32, path_va: usize, stat_va: usize, flags: i32) -> isize {
    let path = match get_path(dirfd, path_va) {
        Some(p) => normalize_path(&p),
        None => return ENOENT,
    };

    // 处理空路径（AT_EMPTY_PATH + dirfd）
    if path == "/" && flags & AT_EMPTY_PATH != 0 {
        // fstat on dirfd
        return sys_fstat(dirfd, stat_va);
    }

    let follow_symlinks = flags & AT_SYMLINK_NOFOLLOW == 0;

    let node = match FS.lookup(&path) {
        Some(n) => n,
        None => return ENOENT,
    };

    let node_guard = node.lock();
    let file_stat = node_guard.stat();
    drop(node_guard);

    let mut stat: Stat = unsafe { core::mem::zeroed() };
    fill_stat(&file_stat, &mut stat);

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(stat_va, bytemuck_cast(core::slice::from_ref(&stat)));
    0
}

pub fn sys_mkdirat(dirfd: i32, path_va: usize, mode: u32) -> isize {
    let path = match get_path(dirfd, path_va) {
        Some(p) => normalize_path(&p),
        None => return ENOENT,
    };

    if FS.lookup(&path).is_some() {
        return EEXIST;
    }

    FS.mkdir_p(&path);
    0
}

pub fn sys_unlinkat(dirfd: i32, path_va: usize, flags: i32) -> isize {
    // 简化实现，不实际删除
    0
}

pub fn sys_getdents64(fd: i32, buf_va: usize, count: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let (inode, _offset) = match t.fds.get(&fd) {
        Some(FileDesc::File { inode, offset, .. }) => (inode.clone(), *offset),
        _ => return EBADF,
    };

    let node = inode.lock();
    let entries = match &node.kind {
        INodeKind::Dir(e) => e.lock(),
        _ => return ENOTDIR,
    };

    // 生成dirent64数组
    let mut buf = Vec::new();
    let mut written = 0usize;

    // 添加 . 和 ..
    let dot_entries: &[(&str, u64, u8)] = &[
        (".", 1, 4),  // DT_DIR
        ("..", 1, 4),
    ];

    for &(name, ino, dtype) in dot_entries {
        let name_bytes = name.as_bytes();
        let rec_len = (19 + name_bytes.len() + 1 + 7) & !7;
        if written + rec_len > count { break; }

        // struct linux_dirent64
        buf.extend_from_slice(&ino.to_le_bytes());      // d_ino
        buf.extend_from_slice(&(written as u64).to_le_bytes()); // d_off
        buf.extend_from_slice(&(rec_len as u16).to_le_bytes()); // d_reclen
        buf.push(dtype); // d_type
        buf.extend_from_slice(name_bytes);
        buf.push(0); // null terminator
        let pad = rec_len - 19 - name_bytes.len() - 1;
        buf.extend(core::iter::repeat(0).take(pad));
        written += rec_len;
    }

    for (name, entry) in entries.iter() {
        let name_bytes = name.as_bytes();
        let rec_len = (19 + name_bytes.len() + 1 + 7) & !7;
        if written + rec_len > count { break; }

        let entry_guard = entry.lock();
        let ino = entry_guard.ino;
        let dtype: u8 = match &entry_guard.kind {
            INodeKind::Dir(_) => 4,
            INodeKind::File(_) => 8,
            INodeKind::Symlink(_) => 10,
            INodeKind::CharDev { .. } => 2,
            INodeKind::BlockDev { .. } => 6,
            _ => 0,
        };
        drop(entry_guard);

        buf.extend_from_slice(&ino.to_le_bytes());
        buf.extend_from_slice(&(written as u64).to_le_bytes());
        buf.extend_from_slice(&(rec_len as u16).to_le_bytes());
        buf.push(dtype);
        buf.extend_from_slice(name_bytes);
        buf.push(0);
        let pad = rec_len - 19 - name_bytes.len() - 1;
        buf.extend(core::iter::repeat(0).take(pad));
        written += rec_len;
    }

    t.memory_set.copy_to_user(buf_va, &buf);

    // 标记已读完（将offset设为EOF）
    drop(t);
    let mut t = task.lock();
    if let Some(FileDesc::File { offset, .. }) = t.fds.get_mut(&fd) {
        *offset = usize::MAX; // 标记为已全部读取
    }

    written as isize
}

pub fn sys_chdir(path_va: usize) -> isize {
    let task = current_task().unwrap();
    let path = {
        let t = task.lock();
        t.memory_set.page_table.read_cstr(path_va)
    };
    let path = normalize_path(&path);

    if FS.lookup(&path).is_none() {
        return ENOENT;
    }

    task.lock().cwd = path;
    0
}

pub fn sys_getcwd(buf_va: usize, size: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();
    let cwd = t.cwd.clone();
    let bytes = cwd.as_bytes();
    if bytes.len() + 1 > size {
        return ERANGE;
    }
    let mut buf = bytes.to_vec();
    buf.push(0);
    t.memory_set.copy_to_user(buf_va, &buf);
    buf_va as isize
}

pub fn sys_faccessat(dirfd: i32, path_va: usize, mode: i32, flags: i32) -> isize {
    let path = match get_path(dirfd, path_va) {
        Some(p) => normalize_path(&p),
        None => return ENOENT,
    };

    // 检查路径是否存在
    match FS.lookup(&path) {
        Some(_) => 0,
        None => ENOENT,
    }
}

pub fn sys_pipe2(pipefd_va: usize, flags: i32) -> isize {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let read_end = FileDesc::Pipe { read_end: true, buf: buf.clone() };
    let write_end = FileDesc::Pipe { read_end: false, buf };

    let task = current_task().unwrap();
    let mut t = task.lock();
    let rfd = t.alloc_fd();
    t.fds.insert(rfd, read_end);
    let wfd = t.alloc_fd();
    t.fds.insert(wfd, write_end);

    let fds = [rfd as u32, wfd as u32];
    t.memory_set.copy_to_user(pipefd_va, bytemuck_cast(&fds));
    0
}

pub fn sys_dup(fd: i32) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();

    let new_fd_info = match t.fds.get(&fd) {
        Some(FileDesc::Stdin) => FileDesc::Stdin,
        Some(FileDesc::Stdout) => FileDesc::Stdout,
        Some(FileDesc::Stderr) => FileDesc::Stderr,
        Some(FileDesc::File { inode, offset, flags }) => FileDesc::File {
            inode: inode.clone(),
            offset: *offset,
            flags: *flags,
        },
        Some(FileDesc::Socket(s)) => FileDesc::Socket(*s),
        None => return EBADF,
        _ => return EINVAL,
    };

    let new_fd = t.alloc_fd();
    t.fds.insert(new_fd, new_fd_info);
    new_fd as isize
}

pub fn sys_dup3(oldfd: i32, newfd: i32, flags: i32) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();

    let new_fd_info = match t.fds.get(&oldfd) {
        Some(FileDesc::Stdin) => FileDesc::Stdin,
        Some(FileDesc::Stdout) => FileDesc::Stdout,
        Some(FileDesc::Stderr) => FileDesc::Stderr,
        Some(FileDesc::File { inode, offset, flags }) => FileDesc::File {
            inode: inode.clone(),
            offset: *offset,
            flags: *flags,
        },
        Some(FileDesc::Socket(s)) => FileDesc::Socket(*s),
        None => return EBADF,
        _ => return EINVAL,
    };

    t.fds.insert(newfd, new_fd_info);
    newfd as isize
}

pub fn sys_readlinkat(dirfd: i32, path_va: usize, buf_va: usize, bufsiz: usize) -> isize {
    let path = match get_path(dirfd, path_va) {
        Some(p) => normalize_path(&p),
        None => return ENOENT,
    };

    // 特殊路径处理
    if path == "/proc/self/exe" {
        let exe = "/usr/sbin/nginx";
        let n = exe.len().min(bufsiz);
        let task = current_task().unwrap();
        let t = task.lock();
        t.memory_set.copy_to_user(buf_va, &exe.as_bytes()[..n]);
        return n as isize;
    }

    let node = match FS.lookup(&path) {
        Some(n) => n,
        None => return ENOENT,
    };

    let node = node.lock();
    match &node.kind {
        INodeKind::Symlink(target) => {
            let n = target.len().min(bufsiz);
            let task = current_task().unwrap();
            let t = task.lock();
            t.memory_set.copy_to_user(buf_va, &target.as_bytes()[..n]);
            n as isize
        }
        _ => EINVAL,
    }
}

pub fn sys_truncate(path_va: usize, length: i64) -> isize {
    let task = current_task().unwrap();
    let path = {
        let t = task.lock();
        normalize_path(&t.memory_set.page_table.read_cstr(path_va))
    };

    if let Some(node) = FS.lookup(&path) {
        let node = node.lock();
        if let INodeKind::File(data) = &node.kind {
            data.lock().resize(length as usize, 0);
            return 0;
        }
    }
    ENOENT
}

pub fn sys_ftruncate(fd: i32, length: i64) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    match t.fds.get(&fd) {
        Some(FileDesc::File { inode, .. }) => {
            let node = inode.lock();
            if let INodeKind::File(data) = &node.kind {
                data.lock().resize(length as usize, 0);
                0
            } else {
                EINVAL
            }
        }
        _ => EBADF,
    }
}

pub fn sys_utimensat(_dirfd: i32, _path_va: usize, _times_va: usize, _flags: i32) -> isize {
    0 // 忽略时间设置
}

#[repr(C)]
struct StatfsResult {
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

pub fn sys_statfs(path_va: usize, buf_va: usize) -> isize {
    let stat = StatfsResult {
        f_type: 0x858458f6, // RAMFS_MAGIC
        f_bsize: 4096,
        f_blocks: 1024 * 1024,
        f_bfree: 512 * 1024,
        f_bavail: 512 * 1024,
        f_files: 65536,
        f_ffree: 32768,
        f_fsid: [0; 2],
        f_namelen: 255,
        f_frsize: 4096,
        f_flags: 0,
        f_spare: [0; 4],
    };
    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(buf_va, bytemuck_cast(core::slice::from_ref(&stat)));
    0
}

pub fn sys_fstatfs(fd: i32, buf_va: usize) -> isize {
    sys_statfs(0, buf_va)
}

pub fn sys_sendfile(out_fd: i32, in_fd: i32, offset_va: usize, count: usize) -> isize {
    // 简化实现
    let task = current_task().unwrap();
    let t = task.lock();

    let (inode, off) = match t.fds.get(&in_fd) {
        Some(FileDesc::File { inode, offset, .. }) => {
            let off = if offset_va != 0 {
                let mut off_buf = [0u8; 8];
                t.memory_set.copy_from_user(offset_va, &mut off_buf);
                i64::from_le_bytes(off_buf) as usize
            } else {
                *offset
            };
            (inode.clone(), off)
        }
        _ => return EBADF,
    };

    let node = inode.lock();
    let data = match &node.kind {
        INodeKind::File(data) => {
            let data = data.lock();
            let available = data.len().saturating_sub(off);
            let n = count.min(available);
            data[off..off + n].to_vec()
        }
        _ => return EINVAL,
    };

    let n = data.len();
    n as isize
}

pub fn sys_symlinkat(target_va: usize, dirfd: i32, path_va: usize) -> isize {
    let task = current_task().unwrap();
    let (target, path) = {
        let t = task.lock();
        let target = t.memory_set.page_table.read_cstr(target_va);
        let path_str = t.memory_set.page_table.read_cstr(path_va);
        let path = if path_str.starts_with('/') {
            path_str
        } else {
            format!("{}/{}", t.cwd, path_str)
        };
        (target, normalize_path(&path))
    };

    FS.create_symlink(&path, &target, 0o777);
    0
}
