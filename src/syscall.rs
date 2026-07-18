//! Linux riscv64 系统调用实现

use crate::fd::{self, EpollEvent, Fd, FdKind, FileFd, Pipe};
use crate::fs::{self, Special, S_IFDIR, S_IFREG};
use crate::mm::{copy_in, copy_in_str, copy_out, MapPerm, VirtAddr};
use crate::task::{self, current_task, Task};
use crate::timer::{get_time_us, get_time};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

mod errno {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EBADF: i32 = 9;
    pub const EAGAIN: i32 = 11;
    pub const ECHILD: i32 = 10;
    pub const ENOMEM: i32 = 12;
    pub const EFAULT: i32 = 14;
    pub const EEXIST: i32 = 17;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENFILE: i32 = 23;
    pub const EMFILE: i32 = 24;
    pub const ENOTTY: i32 = 25;
    pub const EFBIG: i32 = 27;
    pub const EPIPE: i32 = 32;
    pub const ERANGE: i32 = 34;
    pub const ENOSYS: i32 = 38;
    pub const ENOTEMPTY: i32 = 39;
    pub const ELOOP: i32 = 40;
    pub const EAFNOSUPPORT: i32 = 97;
    pub const ENOTSOCK: i32 = 88;
    pub const ECONNRESET: i32 = 104;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNREFUSED: i32 = 111;
}
use errno::*;

const SYSCALL_TRACE: bool = false;

fn cur() -> Arc<Task> {
    current_task().expect("no current task in syscall")
}

// ---------- 用户内存访问 ----------

fn uread_bytes(va: usize, len: usize) -> Result<Vec<u8>, i32> {
    let task = cur();
    let inner = task.inner.lock();
    let mut buf = alloc::vec![0u8; len];
    copy_in(&inner.space, va, &mut buf).map_err(|_| -EFAULT)?;
    Ok(buf)
}

fn uwrite_bytes(va: usize, data: &[u8]) -> Result<(), i32> {
    let task = cur();
    let inner = task.inner.lock();
    copy_out(&inner.space, va, data).map_err(|_| -EFAULT)
}

fn uread_val<T: Copy>(va: usize) -> Result<T, i32> {
    let buf = uread_bytes(va, core::mem::size_of::<T>())?;
    Ok(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const T) })
}

fn uwrite_val<T: Copy>(va: usize, v: &T) -> Result<(), i32> {
    let data =
        unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) };
    uwrite_bytes(va, data)
}

fn uread_str(va: usize) -> Result<String, i32> {
    if va == 0 {
        return Err(-EFAULT);
    }
    let task = cur();
    let inner = task.inner.lock();
    copy_in_str(&inner.space, va).map_err(|_| -EFAULT)
}

fn uread_str_vec(mut va: usize) -> Result<Vec<String>, i32> {
    let mut out = Vec::new();
    if va == 0 {
        return Ok(out);
    }
    loop {
        let p: usize = uread_val(va)?;
        if p == 0 {
            break;
        }
        out.push(uread_str(p)?);
        va += 8;
    }
    Ok(out)
}

// ---------- fd 辅助 ----------

fn get_fd(fd_no: usize) -> Result<Arc<Fd>, i32> {
    cur().get_fd(fd_no).ok_or(-EBADF)
}

fn alloc_fd(fd: Fd) -> Result<usize, i32> {
    let task = cur();
    let id = task.alloc_fd(Arc::new(fd));
    if id == usize::MAX {
        Err(-EMFILE)
    } else {
        Ok(id)
    }
}

/// 阻塞直到闭包返回非 EAGAIN（用于 socket/pipe 读写）
fn block_on<T>(mut f: impl FnMut() -> Result<T, i32>, nonblock: bool) -> Result<T, i32> {
    loop {
        crate::net::poll();
        match f() {
            Err(e) if e == -EAGAIN && !nonblock => {
                task::block_current();
            }
            other => return other,
        }
    }
}

// ---------- 路径解析 ----------

fn resolve_path(path: &str, follow: bool) -> Result<usize, i32> {
    let task = cur();
    let cwd = task.inner.lock().cwd.clone();
    fs::with_fs(|fs| fs.lookup(path, &cwd, follow))
}

fn at_path(dirfd: i32, path: &str) -> String {
    if path.starts_with('/') || dirfd == -100 {
        String::from(path)
    } else {
        // 相对某个 fd 的目录
        match cur().get_fd(dirfd as usize) {
            Some(f) => match &f.kind {
                FdKind::File(file) => alloc::format!("{}/{}", file.path, path),
                _ => String::from(path),
            },
            None => String::from(path),
        }
    }
}

// ---------- stat ----------

fn write_stat(va: usize, node_id: usize) -> Result<isize, i32> {
    let (mode, size) = fs::with_fs(|fs| {
        let n = &fs.nodes[node_id];
        (n.mode, n.size())
    });
    let mut st = [0u8; 128];
    let put64 = |off: usize, v: u64, st: &mut [u8; 128]| st[off..off + 8].copy_from_slice(&v.to_ne_bytes());
    let put32 = |off: usize, v: u32, st: &mut [u8; 128]| st[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    put64(0, 1, &mut st); // st_dev
    put64(8, node_id as u64 + 2, &mut st); // st_ino
    put32(16, mode, &mut st); // st_mode
    put32(20, 1, &mut st); // st_nlink
    put32(24, 0, &mut st); // st_uid
    put32(28, 0, &mut st); // st_gid
    put64(32, 0, &mut st); // st_rdev
    put64(48, size as u64, &mut st); // st_size
    put32(56, 4096, &mut st); // st_blksize
    put64(64, (size as u64 + 511) / 512, &mut st); // st_blocks
    let now_sec = get_time_us() / 1_000_000;
    for off in [72usize, 88, 104] {
        put64(off, now_sec, &mut st);
        put64(off + 8, 0, &mut st);
    }
    uwrite_bytes(va, &st)?;
    Ok(0)
}

// ---------- 主分发 ----------

pub fn syscall(num: usize, args: [usize; 6]) -> isize {
    if SYSCALL_TRACE {
        println!(
            "[sys] pid={} n={} a=({:#x},{:#x},{:#x},{:#x},{:#x},{:#x})",
            cur().pid, num, args[0], args[1], args[2], args[3], args[4], args[5]
        );
    }
    let a = args;
    let ret: Result<isize, i32> = match num {
        17 => sys_getcwd(a[0], a[1]),
        19 => sys_eventfd2(a[0], a[1] as u32),
        20 => sys_epoll_create1(a[0]),
        21 => sys_epoll_ctl(a[0], a[1] as i32, a[2], a[3]),
        22 | 441 => sys_epoll_pwait(a[0], a[1], a[2] as i32, a[3], a[4]),
        23 => sys_dup(a[0]),
        24 => sys_dup3(a[0], a[1], a[2] as u32),
        25 => sys_fcntl(a[0], a[1] as i32, a[2]),
        29 => sys_ioctl(a[0], a[1], a[2]),
        34 => sys_mkdirat(a[0] as i32, a[1], a[2] as u32),
        35 => sys_unlinkat(a[0] as i32, a[1], a[2] as u32),
        36 => sys_symlinkat(a[0], a[1] as i32, a[2]),
        43 | 44 => sys_statfs(a[1]),
        46 => sys_ftruncate(a[0], a[1]),
        47 => Ok(0),  // fallocate
        48 => sys_faccessat(a[0] as i32, a[1]),
        49 => sys_chdir(a[0]),
        50 => sys_fchdir(a[0]),
        52 | 53 | 54 | 55 => Ok(0), // fchmod/fchown
        56 => sys_openat(a[0] as i32, a[1], a[2] as u32, a[3] as u32),
        57 => sys_close(a[0]),
        59 => sys_pipe2(a[0], a[1] as u32),
        61 => sys_getdents64(a[0], a[1], a[2]),
        62 => sys_lseek(a[0], a[1] as i64, a[2] as i32),
        63 => sys_read(a[0], a[1], a[2]),
        64 => sys_write(a[0], a[1], a[2]),
        65 => sys_readv(a[0], a[1], a[2]),
        66 => sys_writev(a[0], a[1], a[2]),
        67 => sys_pread64(a[0], a[1], a[2], a[3]),
        68 => sys_pwrite64(a[0], a[1], a[2], a[3]),
        71 => sys_sendfile(a[0], a[1], a[2], a[3]),
        72 => sys_pselect6(a[0], a[1], a[2], a[3], a[4], a[5]),
        73 => sys_ppoll(a[0], a[1], a[2], a[3]),
        78 => sys_readlinkat(a[0] as i32, a[1], a[2], a[3]),
        79 => sys_fstatat(a[0] as i32, a[1], a[2], a[3] as u32),
        80 => sys_fstat(a[0], a[1]),
        81 | 82 | 83 => Ok(0), // sync/fsync/fdatasync
        88 => Ok(0),           // utimensat
        93 | 94 => task::exit_current(a[0] as i32),
        95 => sys_wait4(-1, a[1], a[2]), // waitid 简化
        96 => {
            cur().inner.lock().clear_child_tid = a[0];
            Ok(cur().pid as isize)
        }
        98 => sys_futex(a[0], a[1] as u32, a[2]),
        99 => Ok(0), // set_robust_list
        101 => sys_nanosleep(a[0]),
        113 => sys_clock_gettime(a[0], a[1]),
        114 => sys_clock_getres(a[1]),
        115 => sys_nanosleep(a[2]), // clock_nanosleep 简化
        124 => {
            task::schedule();
            Ok(0)
        }
        129 | 131 => Ok(0), // kill/tgkill（忽略）
        132 => Ok(0),       // sigaltstack
        134 => sys_rt_sigaction(a[0], a[1], a[2]),
        135 => sys_rt_sigprocmask(a[0] as i32, a[1], a[2]),
        136 => {
            let _ = uwrite_val(a[0], &0u64);
            Ok(0)
        }
        139 => Ok(0), // rt_sigreturn
        140 | 141 => Ok(0),
        144 | 146 | 147 | 149 | 151 | 152 => Ok(0), // set*id
        153 => sys_times(a[0]),
        154 | 157 => Ok(cur().pid as isize), // setpgid/setsid
        155 | 156 => Ok(cur().pid as isize), // getpgid/getsid
        158 => Ok(0),                        // getgroups
        159 => Ok(0),                        // setgroups
        160 => sys_uname(a[0]),
        161 | 162 => Ok(0),
        163 => sys_getrlimit(a[0], a[1]),
        164 => sys_setrlimit(a[0], a[1]),
        165 => {
            let _ = uwrite_bytes(a[0], &[0u8; 144]);
            Ok(0)
        }
        166 => Ok(0o022), // umask
        167 => sys_prctl(a[0] as i32, a[1], a[2], a[3], a[4]),
        168 => Ok(0), // getcpu
        169 => sys_gettimeofday(a[0]),
        172 => Ok(cur().pid as isize),
        173 => Ok(cur()
            .inner
            .lock()
            .parent
            .as_ref()
            .and_then(|p| p.upgrade())
            .map(|p| p.pid as isize)
            .unwrap_or(0)),
        174 | 175 | 176 | 177 => Ok(0),
        178 => Ok(cur().pid as isize), // gettid
        179 => sys_sysinfo(a[0]),
        198 => sys_socket(a[0] as i32, a[1] as i32, a[2] as i32),
        199 => sys_socketpair(a[0] as i32, a[1] as i32, a[2]),
        200 => sys_bind(a[0], a[1], a[2]),
        201 => sys_listen(a[0], a[1] as i32),
        202 => sys_accept4(a[0], a[1], a[2], 0),
        203 => Err(-ECONNREFUSED), // connect（客户端不用）
        204 => sys_getsockname(a[0], a[1], a[2]),
        205 => sys_getpeername(a[0], a[1], a[2]),
        206 => sys_sendto(a[0], a[1], a[2], a[3], a[4], a[5]),
        207 => sys_recvfrom(a[0], a[1], a[2], a[3], a[4], a[5]),
        208 => sys_setsockopt(a[0], a[1] as i32, a[2] as i32, a[3], a[4]),
        209 => sys_getsockopt(a[0], a[1] as i32, a[2] as i32, a[3], a[4]),
        210 => sys_shutdown(a[0], a[1] as i32),
        211 => sys_sendmsg(a[0], a[1]),
        212 => sys_recvmsg(a[0], a[1]),
        213 => Ok(0), // readahead
        214 => sys_brk(a[0]),
        215 => sys_munmap(a[0], a[1]),
        216 => Err(-ENOSYS), // mremap
        220 => sys_clone(a[0], a[1], a[2], a[3], a[4]),
        221 => sys_execve(a[0], a[1], a[2]),
        222 => sys_mmap(a[0], a[1], a[2] as u32, a[3] as u32, a[4] as i32, a[5]),
        226 => sys_mprotect(a[0], a[1], a[2] as u32),
        233 => Ok(0), // madvise
        242 => sys_accept4(a[0], a[1], a[2], a[3] as u32),
        259 => {
            unsafe { core::arch::asm!("fence.i") };
            Ok(0)
        }
        260 => sys_wait4(a[0] as i32, a[1], a[2]),
        261 => sys_prlimit64(a[0], a[1] as u32, a[2], a[3]),
        278 => sys_getrandom(a[0], a[1]),
        283 => Ok(0), // membarrier
        291 => Err(-ENOSYS), // statx
        _ => {
            println!("[syscall] UNIMPLEMENTED num={} args={:#x?}", num, a);
            Err(-ENOSYS)
        }
    };
    let result = match ret {
        Ok(v) => v,
        Err(e) => e as isize,
    };
    if SYSCALL_TRACE {
        println!("[sys] n={} -> {:#x}", num, result);
    }
    result
}

// ================= 文件系统相关 =================

fn sys_getcwd(buf: usize, size: usize) -> Result<isize, i32> {
    let cwd = cur().inner.lock().cwd.clone();
    let bytes = cwd.as_bytes();
    if size < bytes.len() + 1 {
        return Err(-ERANGE);
    }
    uwrite_bytes(buf, bytes)?;
    uwrite_bytes(buf + bytes.len(), &[0])?;
    Ok((bytes.len() + 1) as isize)
}

fn sys_chdir(path_va: usize) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let id = resolve_path(&path, true)?;
    let (is_dir, canonical) = fs::with_fs(|fs| (fs.nodes[id].is_dir(), fs.path_of(id)));
    if !is_dir {
        return Err(-ENOTDIR);
    }
    cur().inner.lock().cwd = canonical;
    Ok(0)
}

fn sys_fchdir(fd_no: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        cur().inner.lock().cwd = file.path.clone();
        Ok(0)
    } else {
        Err(-ENOTDIR)
    }
}

const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;
const O_NONBLOCK: u32 = 0o4000;
const O_DIRECTORY: u32 = 0o200000;
const O_NOFOLLOW: u32 = 0o400000;
const O_CLOEXEC: u32 = 0o2000000;

fn sys_openat(dirfd: i32, path_va: usize, flags: u32, mode: u32) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let full = at_path(dirfd, &path);
    let accmode = flags & 3;
    let readable = accmode != 1;
    let writable = accmode >= 1;

    let node_id = fs::with_fs(|fs| {
        match fs.lookup(&full, "/", flags & O_NOFOLLOW == 0) {
            Ok(id) => Ok(id),
            Err(e) => {
                if e == fs::ENOENT && flags & O_CREAT != 0 {
                    fs.create_file(&full, "/", mode | S_IFREG)
                } else {
                    Err(e)
                }
            }
        }
    })?;

    let (is_dir, is_link, special) = fs::with_fs(|fs| {
        let n = &fs.nodes[node_id];
        (n.is_dir(), n.is_symlink(), n.special)
    });
    if is_link && flags & O_NOFOLLOW != 0 {
        return Err(-ELOOP);
    }
    if flags & O_DIRECTORY != 0 && !is_dir {
        return Err(-ENOTDIR);
    }
    if is_dir && writable {
        return Err(-EISDIR);
    }
    if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
        // 简化：文件已存在则报错
        let existed = fs::with_fs(|fs| fs.lookup(&full, "/", true).is_ok());
        if existed {
            return Err(-EEXIST);
        }
    }
    if flags & O_TRUNC != 0 && writable {
        fs::with_fs(|fs| fs.truncate(node_id, 0));
    }

    let file = FileFd::new(node_id, readable, writable, flags & O_APPEND != 0, full);
    let fd = Fd::new(FdKind::File(file));
    fd.set_nonblock(flags & O_NONBLOCK != 0);
    fd.set_cloexec(flags & O_CLOEXEC != 0);
    let _ = special;
    Ok(alloc_fd(fd)? as isize)
}

fn sys_close(fd_no: usize) -> Result<isize, i32> {
    let task = cur();
    if task.get_fd(fd_no).is_none() {
        return Err(-EBADF);
    }
    task.close_fd(fd_no);
    Ok(0)
}

fn sys_read(fd_no: usize, buf: usize, len: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    match &f.kind {
        FdKind::Stdin => {
            // 阻塞读控制台
            loop {
                if let Some(c) = crate::sbi::console_getchar() {
                    uwrite_bytes(buf, &[c])?;
                    return Ok(1);
                }
                if f.nonblock() {
                    return Err(-EAGAIN);
                }
                task::block_current();
            }
        }
        FdKind::Stdout | FdKind::Stderr => Err(-EBADF),
        FdKind::File(file) => {
            if !file.readable {
                return Err(-EBADF);
            }
            let special = fs::with_fs(|fs| fs.nodes[file.node].special);
            match special {
                Special::Null => Ok(0),
                Special::Zero => {
                    let data = alloc::vec![0u8; len];
                    uwrite_bytes(buf, &data)?;
                    Ok(len as isize)
                }
                Special::Urandom => {
                    let data = random_bytes(len);
                    uwrite_bytes(buf, &data)?;
                    Ok(len as isize)
                }
                Special::None => {
                    let mut off = file.offset.lock();
                    let mut kbuf = alloc::vec![0u8; len];
                    let n = fs::with_fs(|fs| fs.read(file.node, *off, &mut kbuf));
                    *off += n;
                    drop(off);
                    uwrite_bytes(buf, &kbuf[..n])?;
                    Ok(n as isize)
                }
            }
        }
        FdKind::Socket(id) => {
            let id = *id;
            let nonblock = f.nonblock();
            let mut kbuf = alloc::vec![0u8; core::cmp::min(len, 65536)];
            let n = block_on(| | crate::net::recv(id, &mut kbuf), nonblock)?;
            uwrite_bytes(buf, &kbuf[..n])?;
            Ok(n as isize)
        }
        FdKind::PipeRead(pipe) => {
            let pipe = pipe.clone();
            let nonblock = f.nonblock();
            block_on(
                || {
                    let mut p = pipe.inner.lock();
                    if !p.buf.is_empty() {
                        let n = core::cmp::min(len, p.buf.len());
                        let data: Vec<u8> = p.buf.drain(..n).collect();
                        Ok(data)
                    } else if p.write_closed {
                        Ok(Vec::new())
                    } else {
                        Err(-EAGAIN)
                    }
                },
                nonblock,
            )
            .and_then(|data| {
                uwrite_bytes(buf, &data)?;
                Ok(data.len() as isize)
            })
        }
        FdKind::Eventfd(ef) => {
            let nonblock = f.nonblock();
            let ef = ef.clone();
            let v = block_on(
                || {
                    let mut c = ef.count.lock();
                    if *c > 0 {
                        let v = *c;
                        *c = 0;
                        Ok(v)
                    } else {
                        Err(-EAGAIN)
                    }
                },
                nonblock,
            )?;
            if len < 8 {
                return Err(-EINVAL);
            }
            uwrite_bytes(buf, &v.to_ne_bytes())?;
            Ok(8)
        }
        FdKind::UnixStream(pair, is_a) => {
            let pair = pair.clone();
            let is_a = *is_a;
            let nonblock = f.nonblock();
            let data = block_on(
                || {
                    let (inbox, peer_closed) = if is_a {
                        (&pair.b_to_a, &pair.b_closed)
                    } else {
                        (&pair.a_to_b, &pair.a_closed)
                    };
                    let mut ib = inbox.lock();
                    if !ib.is_empty() {
                        let n = core::cmp::min(len, ib.len());
                        Ok(ib.drain(..n).collect::<Vec<u8>>())
                    } else if *peer_closed.lock() {
                        Ok(Vec::new())
                    } else {
                        Err(-EAGAIN)
                    }
                },
                nonblock,
            )?;
            uwrite_bytes(buf, &data)?;
            Ok(data.len() as isize)
        }
        FdKind::Epoll(_) | FdKind::PipeWrite(_) => Err(-EBADF),
    }
}

fn sys_write(fd_no: usize, buf: usize, len: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    match &f.kind {
        FdKind::Stdin => Err(-EBADF),
        FdKind::Stdout | FdKind::Stderr => {
            let data = uread_bytes(buf, len)?;
            for &c in &data {
                if c == b'\n' {
                    crate::sbi::console_putchar(b'\r' as usize);
                }
                crate::sbi::console_putchar(c as usize);
            }
            Ok(len as isize)
        }
        FdKind::File(file) => {
            if !file.writable {
                return Err(-EBADF);
            }
            let special = fs::with_fs(|fs| fs.nodes[file.node].special);
            match special {
                Special::Null => Ok(len as isize),
                Special::Zero => Ok(len as isize),
                Special::Urandom => Ok(len as isize),
                Special::None => {
                    let data = uread_bytes(buf, len)?;
                    let mut off = file.offset.lock();
                    let write_off = if file.append {
                        fs::with_fs(|fs| fs.nodes[file.node].size())
                    } else {
                        *off
                    };
                    let n = fs::with_fs(|fs| fs.write(file.node, write_off, &data));
                    *off = write_off + n;
                    Ok(n as isize)
                }
            }
        }
        FdKind::Socket(id) => {
            let id = *id;
            let nonblock = f.nonblock();
            let data = uread_bytes(buf, len)?;
            let n = block_on(|| crate::net::send(id, &data), nonblock)?;
            Ok(n as isize)
        }
        FdKind::PipeWrite(pipe) => {
            let pipe = pipe.clone();
            let nonblock = f.nonblock();
            let data = uread_bytes(buf, len)?;
            block_on(
                || {
                    let mut p = pipe.inner.lock();
                    if p.read_closed {
                        return Err(-EPIPE);
                    }
                    if p.buf.len() >= p.cap {
                        return Err(-EAGAIN);
                    }
                    let n = core::cmp::min(data.len(), p.cap - p.buf.len());
                    p.buf.extend(&data[..n]);
                    Ok(n)
                },
                nonblock,
            )
            .map(|n| n as isize)
        }
        FdKind::Eventfd(ef) => {
            if len < 8 {
                return Err(-EINVAL);
            }
            let data = uread_bytes(buf, 8)?;
            let v = u64::from_ne_bytes(data.try_into().unwrap());
            if v == u64::MAX {
                return Err(-EINVAL);
            }
            let mut c = ef.count.lock();
            *c = c.saturating_add(v);
            Ok(8)
        }
        FdKind::UnixStream(pair, is_a) => {
            let data = uread_bytes(buf, len)?;
            let (outbox, peer_closed) = if *is_a {
                (&pair.a_to_b, &pair.b_closed)
            } else {
                (&pair.b_to_a, &pair.a_closed)
            };
            if *peer_closed.lock() {
                return Err(-EPIPE);
            }
            outbox.lock().extend(&data);
            Ok(len as isize)
        }
        FdKind::Epoll(_) | FdKind::PipeRead(_) => Err(-EBADF),
    }
}

fn sys_readv(fd_no: usize, iov: usize, cnt: usize) -> Result<isize, i32> {
    let mut total = 0isize;
    for i in 0..cnt {
        let base: usize = uread_val(iov + i * 16)?;
        let len: usize = uread_val(iov + i * 16 + 8)?;
        if len == 0 {
            continue;
        }
        let n = sys_read(fd_no, base, len)?;
        total += n;
        if (n as usize) < len {
            break;
        }
    }
    Ok(total)
}

fn sys_writev(fd_no: usize, iov: usize, cnt: usize) -> Result<isize, i32> {
    let mut total = 0isize;
    for i in 0..cnt {
        let base: usize = uread_val(iov + i * 16)?;
        let len: usize = uread_val(iov + i * 16 + 8)?;
        if len == 0 {
            continue;
        }
        let n = sys_write(fd_no, base, len)?;
        total += n;
        if (n as usize) < len {
            break;
        }
    }
    Ok(total)
}

fn sys_pread64(fd_no: usize, buf: usize, len: usize, off: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        let mut kbuf = alloc::vec![0u8; len];
        let n = fs::with_fs(|fs| fs.read(file.node, off, &mut kbuf));
        uwrite_bytes(buf, &kbuf[..n])?;
        Ok(n as isize)
    } else {
        Err(-EBADF)
    }
}

fn sys_pwrite64(fd_no: usize, buf: usize, len: usize, off: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        let data = uread_bytes(buf, len)?;
        let n = fs::with_fs(|fs| fs.write(file.node, off, &data));
        Ok(n as isize)
    } else {
        Err(-EBADF)
    }
}

fn sys_lseek(fd_no: usize, off: i64, whence: i32) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        let mut cur_off = file.offset.lock();
        let size = fs::with_fs(|fs| fs.nodes[file.node].size()) as i64;
        let new = match whence {
            0 => off,
            1 => *cur_off as i64 + off,
            2 => size + off,
            _ => return Err(-EINVAL),
        };
        if new < 0 {
            return Err(-EINVAL);
        }
        *cur_off = new as usize;
        Ok(new as isize)
    } else {
        Err(-EBADF)
    }
}

fn sys_ftruncate(fd_no: usize, len: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        fs::with_fs(|fs| fs.truncate(file.node, len));
        Ok(0)
    } else {
        Err(-EBADF)
    }
}

fn sys_fstatat(dirfd: i32, path_va: usize, stat_va: usize, flags: u32) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let full = at_path(dirfd, &path);
    let follow = flags & 0x100 == 0; // AT_SYMLINK_NOFOLLOW
    let id = resolve_path(&full, follow)?;
    write_stat(stat_va, id)
}

fn sys_fstat(fd_no: usize, stat_va: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    match &f.kind {
        FdKind::File(file) => write_stat(stat_va, file.node),
        FdKind::Stdout | FdKind::Stderr | FdKind::Stdin => {
            // 字符设备
            let mut st = [0u8; 128];
            st[16..20].copy_from_slice(&(0o20666u32).to_ne_bytes());
            uwrite_bytes(stat_va, &st)?;
            Ok(0)
        }
        FdKind::Socket(_) => {
            let mut st = [0u8; 128];
            st[16..20].copy_from_slice(&(0o140000u32 | 0o777).to_ne_bytes());
            uwrite_bytes(stat_va, &st)?;
            Ok(0)
        }
        _ => {
            let mut st = [0u8; 128];
            st[16..20].copy_from_slice(&(0o10000u32 | 0o644).to_ne_bytes());
            uwrite_bytes(stat_va, &st)?;
            Ok(0)
        }
    }
}

fn sys_faccessat(dirfd: i32, path_va: usize) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let full = at_path(dirfd, &path);
    resolve_path(&full, true)?;
    Ok(0)
}

fn sys_readlinkat(dirfd: i32, path_va: usize, buf: usize, size: usize) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    // 特殊处理 /proc/self/exe
    if path == "/proc/self/exe" {
        let exe = cur().inner.lock().exe.clone();
        let n = core::cmp::min(size, exe.len());
        uwrite_bytes(buf, &exe.as_bytes()[..n])?;
        return Ok(n as isize);
    }
    let full = at_path(dirfd, &path);
    let id = resolve_path(&full, false)?;
    let target = fs::with_fs(|fs| {
        let n = &fs.nodes[id];
        if n.is_symlink() {
            Ok(n.link_target.clone())
        } else {
            Err(-EINVAL)
        }
    })?;
    let n = core::cmp::min(size, target.len());
    uwrite_bytes(buf, &target.as_bytes()[..n])?;
    Ok(n as isize)
}

fn sys_mkdirat(dirfd: i32, path_va: usize, mode: u32) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let full = at_path(dirfd, &path);
    fs::with_fs(|fs| fs.mkdir(&full, "/", mode))?;
    Ok(0)
}

fn sys_unlinkat(dirfd: i32, path_va: usize, flags: u32) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let full = at_path(dirfd, &path);
    if flags & 0x200 != 0 {
        fs::with_fs(|fs| fs.rmdir(&full, "/"))?;
    } else {
        fs::with_fs(|fs| fs.unlink(&full, "/"))?;
    }
    Ok(0)
}

fn sys_symlinkat(target_va: usize, dirfd: i32, link_va: usize) -> Result<isize, i32> {
    let target = uread_str(target_va)?;
    let link = uread_str(link_va)?;
    let full = at_path(dirfd, &link);
    fs::with_fs(|fs| fs.create_symlink(&full, "/", &target))?;
    Ok(0)
}

fn sys_getdents64(fd_no: usize, buf: usize, size: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    if let FdKind::File(file) = &f.kind {
        let entries = fs::with_fs(|fs| fs.readdir(file.node));
        let mut off = file.offset.lock();
        let mut written = 0usize;
        let mut idx = *off;
        while idx < entries.len() {
            let (name, ftype) = &entries[idx];
            let reclen = (19 + name.len() + 1 + 7) / 8 * 8;
            if written + reclen > size {
                break;
            }
            let mut rec = alloc::vec![0u8; reclen];
            rec[0..8].copy_from_slice(&(idx as u64 + 2).to_ne_bytes()); // d_ino
            rec[8..16].copy_from_slice(&((idx as i64 + 1)).to_ne_bytes()); // d_off
            rec[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
            rec[18] = match *ftype {
                S_IFDIR => 4,
                S_IFREG => 8,
                _ => 10,
            };
            rec[19..19 + name.len()].copy_from_slice(name.as_bytes());
            uwrite_bytes(buf + written, &rec)?;
            written += reclen;
            idx += 1;
        }
        *off = idx;
        Ok(written as isize)
    } else {
        Err(-ENOTDIR)
    }
}

fn sys_statfs(buf: usize) -> Result<isize, i32> {
    let mut st = [0u8; 120];
    st[0..8].copy_from_slice(&0x01021994u64.to_ne_bytes()); // tmpfs magic
    st[8..16].copy_from_slice(&4096u64.to_ne_bytes());
    uwrite_bytes(buf, &st)?;
    Ok(0)
}

fn sys_dup(old: usize) -> Result<isize, i32> {
    let f = get_fd(old)?;
    let task = cur();
    let id = task.alloc_fd(f);
    if id == usize::MAX {
        Err(-EMFILE)
    } else {
        Ok(id as isize)
    }
}

fn sys_dup3(old: usize, new: usize, flags: u32) -> Result<isize, i32> {
    if old == new {
        return Err(-EINVAL);
    }
    let f = get_fd(old)?;
    let task = cur();
    task.close_fd(new);
    let id = task.alloc_fd_from(new, f.clone());
    if id != new {
        return Err(-EMFILE);
    }
    f.set_cloexec(flags & O_CLOEXEC != 0);
    Ok(new as isize)
}

fn sys_fcntl(fd_no: usize, cmd: i32, arg: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    match cmd {
        0 => {
            // F_DUPFD
            let task = cur();
            let id = task.alloc_fd_from(arg, f);
            if id == usize::MAX {
                Err(-EMFILE)
            } else {
                Ok(id as isize)
            }
        }
        1030 => {
            // F_DUPFD_CLOEXEC
            let task = cur();
            let id = task.alloc_fd_from(arg, f.clone());
            if id == usize::MAX {
                Err(-EMFILE)
            } else {
                f.set_cloexec(true);
                Ok(id as isize)
            }
        }
        1 => Ok(f.cloexec() as isize), // F_GETFD
        2 => {
            f.set_cloexec(arg & 1 != 0);
            Ok(0)
        }
        3 => {
            // F_GETFL
            let mut flags = 0usize;
            if f.nonblock() {
                flags |= O_NONBLOCK as usize;
            }
            if let FdKind::File(file) = &f.kind {
                let acc = match (file.readable, file.writable) {
                    (true, false) => 0,
                    (false, true) => 1,
                    _ => 2,
                };
                flags |= acc;
                if file.append {
                    flags |= O_APPEND as usize;
                }
            }
            Ok(flags as isize)
        }
        4 => {
            // F_SETFL
            f.set_nonblock(arg as u32 & O_NONBLOCK != 0);
            Ok(0)
        }
        5 | 6 | 7 => Ok(0), // F_GETLK/F_SETLK/F_SETLKW
        _ => Err(-EINVAL),
    }
}

fn sys_ioctl(fd_no: usize, req: usize, arg: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    match req {
        0x5421 => {
            // FIONBIO
            let v: i32 = uread_val(arg)?;
            f.set_nonblock(v != 0);
            Ok(0)
        }
        0x5413 => {
            // TIOCGWINSZ
            let ws: [u16; 4] = [24, 80, 0, 0];
            uwrite_bytes(arg, unsafe {
                core::slice::from_raw_parts(ws.as_ptr() as *const u8, 8)
            })?;
            Ok(0)
        }
        0x541B => {
            // FIONREAD
            let n = if let FdKind::Socket(id) = &f.kind {
                crate::net::recv_available(*id)
            } else {
                0
            };
            uwrite_val(arg, &(n as i32))?;
            Ok(0)
        }
        0x5401 | 0x5402 => Err(-ENOTTY), // TCGETS/TCSETS
        _ => Err(-ENOTTY),
    }
}

fn sys_pipe2(pipefd: usize, flags: u32) -> Result<isize, i32> {
    let pipe = Arc::new(Pipe::new());
    let rfd = Fd::new(FdKind::PipeRead(pipe.clone()));
    let wfd = Fd::new(FdKind::PipeWrite(pipe));
    if flags & O_NONBLOCK != 0 {
        rfd.set_nonblock(true);
        wfd.set_nonblock(true);
    }
    if flags & O_CLOEXEC != 0 {
        rfd.set_cloexec(true);
        wfd.set_cloexec(true);
    }
    let r = alloc_fd(rfd)?;
    let w = alloc_fd(wfd)?;
    uwrite_val(pipefd, &(r as i32))?;
    uwrite_val(pipefd + 4, &(w as i32))?;
    Ok(0)
}

fn sys_sendfile(out_fd: usize, in_fd: usize, offset_va: usize, count: usize) -> Result<isize, i32> {
    let in_f = get_fd(in_fd)?;
    let mut off = if offset_va != 0 {
        uread_val::<usize>(offset_va)?
    } else {
        match &in_f.kind {
            FdKind::File(file) => *file.offset.lock(),
            _ => return Err(-EINVAL),
        }
    };
    let mut total = 0usize;
    while total < count {
        let chunk = core::cmp::min(65536, count - total);
        let mut kbuf = alloc::vec![0u8; chunk];
        let n = match &in_f.kind {
            FdKind::File(file) => fs::with_fs(|fs| fs.read(file.node, off, &mut kbuf)),
            _ => return Err(-EINVAL),
        };
        if n == 0 {
            break;
        }
        // 直接写 socket
        let out_f = get_fd(out_fd)?;
        let written = match &out_f.kind {
            FdKind::Socket(id) => {
                let id = *id;
                let nb = out_f.nonblock();
                block_on(|| crate::net::send(id, &kbuf[..n]), nb)?
            }
            _ => return Err(-EINVAL),
        };
        off += written;
        total += written;
        if written < n {
            break;
        }
    }
    if offset_va != 0 {
        uwrite_val(offset_va, &off)?;
    } else if let FdKind::File(file) = &in_f.kind {
        *file.offset.lock() = off;
    }
    Ok(total as isize)
}

// ================= 进程相关 =================

fn sys_brk(addr: usize) -> Result<isize, i32> {
    let task = cur();
    let mut inner = task.inner.lock();
    if addr == 0 {
        return Ok(inner.brk as isize);
    }
    let old = inner.brk;
    if addr < inner.brk_start {
        return Ok(old as isize);
    }
    let page = crate::config::PAGE_SIZE;
    let old_ceil = (old + page - 1) / page * page;
    let new_ceil = (addr + page - 1) / page * page;
    if new_ceil > old_ceil {
        let area = crate::mm::MapArea::new(
            VirtAddr(old_ceil),
            VirtAddr(new_ceil),
            MapPerm::R | MapPerm::W | MapPerm::U,
        );
        inner.space.map_area(area, None);
    } else if new_ceil < old_ceil {
        inner.space.unmap_range(VirtAddr(new_ceil), VirtAddr(old_ceil));
    }
    inner.brk = addr;
    Ok(addr as isize)
}

fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd_no: i32,
    offset: usize,
) -> Result<isize, i32> {
    if len == 0 {
        return Err(-EINVAL);
    }
    let task = cur();
    let mut perm = MapPerm::U;
    if prot & 1 != 0 {
        perm |= MapPerm::R;
    }
    if prot & 2 != 0 {
        perm |= MapPerm::W;
    }
    if prot & 4 != 0 {
        perm |= MapPerm::X;
    }
    let page = crate::config::PAGE_SIZE;
    let map_fixed = flags & 0x10 != 0;
    let anonymous = flags & 0x20 != 0;

    // 先读文件数据（get_fd 会加 inner 锁）
    let data = if !anonymous && fd_no >= 0 {
        let f = cur().get_fd(fd_no as usize).ok_or(-EBADF)?;
        match &f.kind {
            FdKind::File(file) => {
                let mut buf = alloc::vec![0u8; len];
                let n = fs::with_fs(|fs| fs.read(file.node, offset, &mut buf));
                buf.truncate(n);
                Some(buf)
            }
            _ => return Err(-EBADF),
        }
    } else {
        None
    };

    let task = cur();
    let mut inner = task.inner.lock();
    let start = if map_fixed && addr != 0 {
        // 解除已有映射
        inner
            .space
            .unmap_range(VirtAddr(addr), VirtAddr(addr + len));
        addr
    } else if addr != 0 && inner.space.range_free(
        VirtAddr(addr).floor(),
        VirtAddr(addr + len).ceil(),
    ) {
        addr & !(page - 1)
    } else {
        let top = inner.mmap_top;
        inner.mmap_top += (len + page - 1) / page * page;
        top
    };

    let area = crate::mm::MapArea::new(VirtAddr(start), VirtAddr(start + len), perm);
    inner.space.map_area(area, None);
    if let Some(d) = data {
        copy_out(&inner.space, start, &d).map_err(|_| -EFAULT)?;
    }
    Ok(start as isize)
}

fn sys_munmap(addr: usize, len: usize) -> Result<isize, i32> {
    if addr % crate::config::PAGE_SIZE != 0 {
        return Err(-EINVAL);
    }
    let task = cur();
    let mut inner = task.inner.lock();
    inner.space.unmap_range(VirtAddr(addr), VirtAddr(addr + len));
    Ok(0)
}

fn sys_mprotect(addr: usize, len: usize, prot: u32) -> Result<isize, i32> {
    let task = cur();
    let mut perm = MapPerm::U;
    if prot & 1 != 0 {
        perm |= MapPerm::R;
    }
    if prot & 2 != 0 {
        perm |= MapPerm::W;
    }
    if prot & 4 != 0 {
        perm |= MapPerm::X;
    }
    let mut inner = task.inner.lock();
    inner.space.protect_range(VirtAddr(addr), VirtAddr(addr + len), perm);
    Ok(0)
}

fn sys_clone(
    flags: usize,
    _stack: usize,
    _ptid: usize,
    _ctid: usize,
    _tls: usize,
) -> Result<isize, i32> {
    // 只支持 fork 语义
    if flags != 0 && flags != 17 {
        println!("[clone] unsupported flags={:#x}", flags);
        return Err(-ENOSYS);
    }
    let parent = cur();
    let (space, fd_table, cwd, exe, brk_start, brk, mmap_top, sig_actions, sig_mask, name) = {
        let inner = parent.inner.lock();
        (
            inner.space.fork_copy(),
            inner.fd_table.clone(),
            inner.cwd.clone(),
            inner.exe.clone(),
            inner.brk_start,
            inner.brk,
            inner.mmap_top,
            inner.sig_actions,
            inner.sig_mask,
            inner.name.clone(),
        )
    };
    let child = task::new_task(space, name);
    {
        let mut inner = child.inner.lock();
        inner.fd_table = fd_table;
        inner.cwd = cwd;
        inner.exe = exe;
        inner.brk_start = brk_start;
        inner.brk = brk;
        inner.mmap_top = mmap_top;
        inner.sig_actions = sig_actions;
        inner.sig_mask = sig_mask;
        inner.parent = Some(Arc::downgrade(&parent));
    }
    // 复制 trap context（用户寄存器），子进程返回 0
    let pcx = parent.trap_cx();
    let ccx = child.trap_cx();
    ccx.x = pcx.x;
    ccx.sstatus = pcx.sstatus;
    ccx.sepc = pcx.sepc;
    ccx.x[10] = 0;
    parent.inner.lock().children.push(child.clone());
    task::add_to_queue(child.clone());
    Ok(child.pid as isize)
}

fn sys_execve(path_va: usize, argv_va: usize, envp_va: usize) -> Result<isize, i32> {
    let path = uread_str(path_va)?;
    let args = uread_str_vec(argv_va)?;
    let envs = uread_str_vec(envp_va)?;
    let full = at_path(-100, &path);
    let data = fs::with_fs(|fs| match fs.lookup(&full, "/", true) {
        Ok(id) => {
            if fs.nodes[id].is_dir() {
                Err(-EISDIR)
            } else {
                Ok(fs.nodes[id].data.clone())
            }
        }
        Err(e) => Err(e),
    })?;
    let img = crate::elf::build_image(&data, args, envs).map_err(|_| -EINVAL)?;
    let task = cur();
    let mut img_space = img.space;
    {
        let mut inner = task.inner.lock();
        // 关闭 cloexec fd
        for slot in inner.fd_table.iter_mut() {
            if let Some(f) = slot {
                if f.cloexec() {
                    *slot = None;
                }
            }
        }
        // 新地址空间需要映射当前内核栈顶页到 TRAP_CONTEXT
        let kstack_top_ppn = inner.kstack.top_page_ppn();
        img_space.map_page_at(
            VirtAddr(crate::config::TRAP_CONTEXT),
            kstack_top_ppn,
            MapPerm::R | MapPerm::W,
        );
        let old_space = core::mem::replace(&mut inner.space, img_space);
        drop(old_space);
        inner.brk_start = img.brk;
        inner.brk = img.brk;
        inner.mmap_top = crate::config::MMAP_BASE;
        inner.exe = img.name.clone();
        inner.name = img.name;
    }
    // 重置当前任务的 trap context
    let ccx = task.trap_cx();
    ccx.x = [0; 32];
    ccx.sepc = img.entry;
    ccx.x[2] = img.sp;
    unsafe { core::arch::asm!("fence.i") };
    Ok(0)
}

fn sys_wait4(pid: i32, status_va: usize, _options: usize) -> Result<isize, i32> {
    loop {
        let task = cur();
        let mut inner = task.inner.lock();
        let mut found: Option<usize> = None;
        for (i, child) in inner.children.iter().enumerate() {
            if pid == -1 || child.pid as i32 == pid {
                if child.state() == task::TaskState::Zombie {
                    found = Some(i);
                    break;
                }
            }
        }
        if let Some(i) = found {
            let child = inner.children.remove(i);
            drop(inner);
            let code = child.inner.lock().exit_code;
            let cpid = child.pid;
            task::remove_task(cpid);
            if status_va != 0 {
                uwrite_val(status_va, &(code << 8))?;
            }
            return Ok(cpid as isize);
        }
        if inner.children.is_empty() {
            return Err(-ECHILD);
        }
        drop(inner);
        task::block_current();
    }
}

fn sys_futex(uaddr: usize, op: u32, val: usize) -> Result<isize, i32> {
    let cmd = op & 0x7f;
    match cmd {
        0 => {
            // FUTEX_WAIT
            let cur_val: u32 = uread_val(uaddr)?;
            if cur_val != val as u32 {
                return Err(-EAGAIN);
            }
            task::block_current();
            Ok(0)
        }
        1 => Ok(0),  // FUTEX_WAKE
        _ => Ok(0),
    }
}

fn sys_nanosleep(req_va: usize) -> Result<isize, i32> {
    let sec: u64 = uread_val(req_va)?;
    let nsec: u64 = uread_val(req_va + 8)?;
    let us = sec * 1_000_000 + nsec / 1000;
    let deadline = get_time_us() + us;
    task::sleep_current_until(deadline);
    Ok(0)
}

/// CLOCK_REALTIME 相对开机时间的偏移（2026-07-18 左右的 Unix 时间戳）
const REALTIME_OFFSET_SEC: u64 = 1_784_300_000;

fn sys_clock_gettime(clock: usize, ts_va: usize) -> Result<isize, i32> {
    let us = get_time_us();
    let offset = if clock == 0 { REALTIME_OFFSET_SEC } else { 0 };
    uwrite_val(ts_va, &(us / 1_000_000 + offset))?;
    uwrite_val(ts_va + 8, &((us % 1_000_000) * 1000))?;
    Ok(0)
}

fn sys_clock_getres(ts_va: usize) -> Result<isize, i32> {
    if ts_va != 0 {
        uwrite_val(ts_va, &0u64)?;
        uwrite_val(ts_va + 8, &1u64)?;
    }
    Ok(0)
}

fn sys_gettimeofday(tv_va: usize) -> Result<isize, i32> {
    if tv_va != 0 {
        let us = get_time_us();
        uwrite_val(tv_va, &(us / 1_000_000 + REALTIME_OFFSET_SEC))?;
        uwrite_val(tv_va + 8, &(us % 1_000_000))?;
    }
    Ok(0)
}

fn sys_times(buf: usize) -> Result<isize, i32> {
    let t = [0u64; 4];
    uwrite_bytes(buf, unsafe {
        core::slice::from_raw_parts(t.as_ptr() as *const u8, 32)
    })?;
    Ok((get_time() / (crate::config::CLOCK_FREQ / 100)) as isize)
}

fn sys_uname(buf: usize) -> Result<isize, i32> {
    let mut uts = [0u8; 65 * 6];
    let fields: [&str; 6] = [
        "Linux",
        "ijiege-k3",
        "6.1.0-ijiege",
        "#1 SMP",
        "riscv64",
        "(none)",
    ];
    for (i, f) in fields.iter().enumerate() {
        uts[i * 65..i * 65 + f.len()].copy_from_slice(f.as_bytes());
    }
    uwrite_bytes(buf, &uts)?;
    Ok(0)
}

fn sys_sysinfo(buf: usize) -> Result<isize, i32> {
    let mut info = [0u8; 112];
    let uptime = get_time_us() / 1_000_000;
    info[0..8].copy_from_slice(&(uptime as i64).to_ne_bytes());
    let total = 256u64 * 1024 * 1024;
    info[32..40].copy_from_slice(&total.to_ne_bytes()); // totalram
    info[40..48].copy_from_slice(&(total / 2).to_ne_bytes()); // freeram
    info[104..106].copy_from_slice(&1u16.to_ne_bytes()); // procs
    uwrite_bytes(buf, &info)?;
    Ok(0)
}

fn sys_getrlimit(resource: usize, rlim_va: usize) -> Result<isize, i32> {
    let task = cur();
    let inner = task.inner.lock();
    let (cur, max) = match resource {
        3 => (inner.rlimit_stack, inner.rlimit_stack),       // RLIMIT_STACK
        7 => (inner.rlimit_nofile, inner.rlimit_nofile),     // RLIMIT_NOFILE
        _ => (u64::MAX, u64::MAX),
    };
    drop(inner);
    uwrite_val(rlim_va, &cur)?;
    uwrite_val(rlim_va + 8, &max)?;
    Ok(0)
}

fn sys_setrlimit(resource: usize, rlim_va: usize) -> Result<isize, i32> {
    let cur_v: u64 = uread_val(rlim_va)?;
    let task = cur();
    let mut inner = task.inner.lock();
    match resource {
        3 => inner.rlimit_stack = cur_v,
        7 => inner.rlimit_nofile = cur_v,
        _ => {}
    }
    Ok(0)
}

fn sys_prlimit64(_pid: usize, resource: u32, new_va: usize, old_va: usize) -> Result<isize, i32> {
    if old_va != 0 {
        sys_getrlimit(resource as usize, old_va)?;
    }
    if new_va != 0 {
        sys_setrlimit(resource as usize, new_va)?;
    }
    Ok(0)
}

fn sys_rt_sigaction(sig: usize, act_va: usize, old_va: usize) -> Result<isize, i32> {
    if sig == 0 || sig > 64 {
        return Err(-EINVAL);
    }
    // 先读用户内存（uread 内部会加 inner 锁，不能持锁调用）
    let act = if act_va != 0 {
        Some(uread_val::<crate::signal::SigAction>(act_va)?)
    } else {
        None
    };
    let task = cur();
    let mut inner = task.inner.lock();
    let old = inner.sig_actions[sig];
    if let Some(act) = act {
        inner.sig_actions[sig] = act;
    }
    drop(inner);
    if old_va != 0 {
        uwrite_val(old_va, &old)?;
    }
    Ok(0)
}

fn sys_rt_sigprocmask(how: i32, set_va: usize, old_va: usize) -> Result<isize, i32> {
    let set = if set_va != 0 {
        Some(uread_val::<u64>(set_va)?)
    } else {
        None
    };
    let task = cur();
    let mut inner = task.inner.lock();
    let old = inner.sig_mask;
    if let Some(set) = set {
        match how {
            0 => inner.sig_mask |= set,   // SIG_BLOCK
            1 => inner.sig_mask &= !set,  // SIG_UNBLOCK
            2 => inner.sig_mask = set,    // SIG_SETMASK
            _ => return Err(-EINVAL),
        }
    }
    drop(inner);
    if old_va != 0 {
        uwrite_val(old_va, &old)?;
    }
    Ok(0)
}

fn sys_prctl(option: i32, arg2: usize, _a3: usize, _a4: usize, _a5: usize) -> Result<isize, i32> {
    match option {
        15 => {
            // PR_SET_NAME
            let name = uread_str(arg2).unwrap_or_default();
            cur().inner.lock().name = name;
            Ok(0)
        }
        16 => {
            // PR_GET_NAME
            let name = cur().inner.lock().name.clone();
            let mut buf = [0u8; 16];
            let n = core::cmp::min(15, name.len());
            buf[..n].copy_from_slice(&name.as_bytes()[..n]);
            uwrite_bytes(arg2, &buf)?;
            Ok(0)
        }
        _ => Ok(0),
    }
}

fn sys_getrandom(buf: usize, len: usize) -> Result<isize, i32> {
    let data = random_bytes(len);
    uwrite_bytes(buf, &data)?;
    Ok(len as isize)
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = alloc::vec![0u8; len];
    let mut x = get_time() | 1;
    for chunk in out.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let bytes = x.to_ne_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

// ================= epoll / poll =================

fn sys_epoll_create1(_flags: usize) -> Result<isize, i32> {
    let id = fd::epoll_create();
    Ok(alloc_fd(Fd::new(FdKind::Epoll(id)))? as isize)
}

fn sys_epoll_ctl(epfd: usize, op: i32, fd_no: usize, event_va: usize) -> Result<isize, i32> {
    let f = get_fd(epfd)?;
    let epoll_id = match &f.kind {
        FdKind::Epoll(id) => *id,
        _ => return Err(-EBADF),
    };
    // 确认目标 fd 存在
    get_fd(fd_no)?;
    let event = if event_va != 0 {
        let events: u32 = uread_val(event_va)?;
        let data: u64 = uread_val(event_va + 8)?;
        Some(EpollEvent { events, data })
    } else {
        None
    };
    Ok(fd::epoll_ctl(epoll_id, op, fd_no, event) as isize)
}

fn sys_epoll_pwait(
    epfd: usize,
    events_va: usize,
    maxevents: i32,
    timeout_ms: usize,
    _sigmask: usize,
) -> Result<isize, i32> {
    let f = get_fd(epfd)?;
    let epoll_id = match &f.kind {
        FdKind::Epoll(id) => *id,
        _ => return Err(-EBADF),
    };
    let max = core::cmp::max(1, maxevents) as usize;
    let timeout = timeout_ms as isize;
    let deadline = if timeout >= 0 {
        get_time_us() + timeout as u64 * 1000
    } else {
        u64::MAX
    };
    loop {
        crate::net::poll();
        let mut out = Vec::new();
        let task = cur();
        let n = {
            let inner = task.inner.lock();
            fd::epoll_collect(epoll_id, &inner.fd_table, &mut out, max)
        };
        if n > 0 {
            for (i, ev) in out.iter().enumerate() {
                uwrite_val(events_va + i * 16, &ev.events)?;
                uwrite_val(events_va + i * 16 + 8, &ev.data)?;
            }
            return Ok(n as isize);
        }
        if get_time_us() >= deadline {
            return Ok(0);
        }
        task::block_current();
    }
}

fn sys_pselect6(
    nfds: usize,
    readfds: usize,
    writefds: usize,
    _exceptfds: usize,
    timeout_va: usize,
    _sigmask: usize,
) -> Result<isize, i32> {
    let deadline = if timeout_va != 0 {
        let sec: u64 = uread_val(timeout_va)?;
        let nsec: u64 = uread_val(timeout_va + 8)?;
        get_time_us() + sec * 1_000_000 + nsec / 1000
    } else {
        u64::MAX
    };
    let read_set = if readfds != 0 {
        uread_bytes(readfds, nfds / 8 + 1)?
    } else {
        Vec::new()
    };
    let write_set = if writefds != 0 {
        uread_bytes(writefds, nfds / 8 + 1)?
    } else {
        Vec::new()
    };
    loop {
        crate::net::poll();
        let mut count = 0isize;
        let mut r_out = alloc::vec![0u8; read_set.len()];
        let mut w_out = alloc::vec![0u8; write_set.len()];
        let task = cur();
        let inner = task.inner.lock();
        for fd_no in 0..nfds {
            let want_r = !read_set.is_empty() && read_set[fd_no / 8] & (1 << (fd_no % 8)) != 0;
            let want_w = !write_set.is_empty() && write_set[fd_no / 8] & (1 << (fd_no % 8)) != 0;
            if !want_r && !want_w {
                continue;
            }
            if let Some(Some(f)) = inner.fd_table.get(fd_no) {
                let (r, w, e) = f.poll();
                if (want_r && (r || e)) || (want_w && (w || e)) {
                    if want_r && (r || e) {
                        r_out[fd_no / 8] |= 1 << (fd_no % 8);
                    }
                    if want_w && (w || e) {
                        w_out[fd_no / 8] |= 1 << (fd_no % 8);
                    }
                    count += 1;
                }
            }
        }
        drop(inner);
        if count > 0 {
            if readfds != 0 {
                uwrite_bytes(readfds, &r_out)?;
            }
            if writefds != 0 {
                uwrite_bytes(writefds, &w_out)?;
            }
            return Ok(count);
        }
        if get_time_us() >= deadline {
            if readfds != 0 {
                uwrite_bytes(readfds, &r_out)?;
            }
            if writefds != 0 {
                uwrite_bytes(writefds, &w_out)?;
            }
            return Ok(0);
        }
        task::block_current();
    }
}

fn sys_ppoll(fds_va: usize, nfds: usize, timeout_va: usize, _sigmask: usize) -> Result<isize, i32> {
    // struct pollfd { int fd; short events; short revents; }
    let deadline = if timeout_va != 0 {
        let sec: i64 = uread_val(timeout_va)?;
        let nsec: i64 = uread_val(timeout_va + 8)?;
        if sec < 0 {
            u64::MAX
        } else {
            get_time_us() + sec as u64 * 1_000_000 + nsec as u64 / 1000
        }
    } else {
        u64::MAX
    };
    loop {
        crate::net::poll();
        let mut count = 0isize;
        let task = cur();
        let inner = task.inner.lock();
        for i in 0..nfds {
            let base = fds_va + i * 8;
            let fd_no: i32 = uread_val(base)?;
            if fd_no < 0 {
                continue;
            }
            let events: i16 = uread_val(base + 4)?;
            let mut revents = 0i16;
            if let Some(Some(f)) = inner.fd_table.get(fd_no as usize) {
                let (r, w, e) = f.poll();
                if r && events & 0x1 != 0 {
                    revents |= 0x1;
                }
                if w && events & 0x4 != 0 {
                    revents |= 0x4;
                }
                if e {
                    revents |= 0x8 | 0x10;
                }
            } else {
                revents = 0x20; // POLLNVAL
            }
            if revents != 0 {
                count += 1;
            }
            let _ = uwrite_val(base + 6, &revents);
        }
        drop(inner);
        if count > 0 {
            return Ok(count);
        }
        if get_time_us() >= deadline {
            return Ok(0);
        }
        task::block_current();
    }
}

// ================= socket =================

fn parse_sockaddr(va: usize) -> Result<([u8; 4], u16), i32> {
    let _family: u16 = uread_val(va)?;
    let port_be: u16 = uread_val(va + 2)?;
    let addr: [u8; 4] = uread_val(va + 4)?;
    Ok((addr, u16::from_be(port_be)))
}

fn write_sockaddr(va: usize, len_va: usize, ip: [u8; 4], port: u16) -> Result<(), i32> {
    let mut sa = [0u8; 16];
    sa[0..2].copy_from_slice(&2u16.to_ne_bytes()); // AF_INET
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip);
    uwrite_bytes(va, &sa)?;
    if len_va != 0 {
        uwrite_val(len_va, &16u32)?;
    }
    Ok(())
}

fn sys_eventfd2(initval: usize, flags: u32) -> Result<isize, i32> {
    let fd = Fd::new(FdKind::Eventfd(Arc::new(fd::Eventfd::new(initval as u64))));
    fd.set_nonblock(flags & O_NONBLOCK != 0);
    fd.set_cloexec(flags & O_CLOEXEC != 0);
    Ok(alloc_fd(fd)? as isize)
}

fn sys_socketpair(domain: i32, stype: i32, sv_va: usize) -> Result<isize, i32> {
    if domain != 1 {
        return Err(-EAFNOSUPPORT);
    }
    let pair = Arc::new(fd::UnixStream::new());
    let fda = Fd::new(FdKind::UnixStream(pair.clone(), true));
    let fdb = Fd::new(FdKind::UnixStream(pair, false));
    if stype as u32 & O_NONBLOCK != 0 {
        fda.set_nonblock(true);
        fdb.set_nonblock(true);
    }
    if stype as u32 & O_CLOEXEC != 0 {
        fda.set_cloexec(true);
        fdb.set_cloexec(true);
    }
    let a = alloc_fd(fda)?;
    let b = alloc_fd(fdb)?;
    uwrite_val(sv_va, &(a as i32))?;
    uwrite_val(sv_va + 4, &(b as i32))?;
    Ok(0)
}

fn sys_socket(domain: i32, stype: i32, _protocol: i32) -> Result<isize, i32> {
    if domain != 2 {
        return Err(-EAFNOSUPPORT);
    }
    let type_bits = stype & 0xf;
    if type_bits != 1 {
        return Err(-EAFNOSUPPORT); // 只支持 SOCK_STREAM
    }
    let id = crate::net::tcp_socket().ok_or(-ENOMEM)?;
    let fd = Fd::new(FdKind::Socket(id));
    fd.set_nonblock(stype & O_NONBLOCK as i32 != 0);
    fd.set_cloexec(stype & O_CLOEXEC as i32 != 0);
    Ok(alloc_fd(fd)? as isize)
}

fn sys_bind(fd_no: usize, addr_va: usize, _len: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    let (ip, port) = parse_sockaddr(addr_va)?;
    Ok(crate::net::bind(id, ip, port) as isize)
}

fn sys_listen(fd_no: usize, backlog: i32) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    Ok(crate::net::listen(id, backlog) as isize)
}

fn sys_accept4(fd_no: usize, addr_va: usize, len_va: usize, flags: u32) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    let nonblock = f.nonblock();
    let (new_id, ip, port) = block_on(|| crate::net::accept(id), nonblock)?;
    let new_fd = Fd::new(FdKind::Socket(new_id));
    new_fd.set_nonblock(flags & O_NONBLOCK != 0);
    new_fd.set_cloexec(flags & O_CLOEXEC != 0);
    let new_fd_no = alloc_fd(new_fd)?;
    if addr_va != 0 {
        write_sockaddr(addr_va, len_va, ip, port)?;
    }
    Ok(new_fd_no as isize)
}

fn sys_getsockname(fd_no: usize, addr_va: usize, len_va: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    let (ip, port) = crate::net::getsockname(id).unwrap_or(([0, 0, 0, 0], 0));
    write_sockaddr(addr_va, len_va, ip, port)?;
    Ok(0)
}

fn sys_getpeername(fd_no: usize, addr_va: usize, len_va: usize) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    match crate::net::getpeername(id) {
        Some((ip, port)) => {
            write_sockaddr(addr_va, len_va, ip, port)?;
            Ok(0)
        }
        None => Err(-107), // ENOTCONN
    }
}

fn sys_sendto(
    fd_no: usize,
    buf: usize,
    len: usize,
    _flags: usize,
    _addr: usize,
    _addrlen: usize,
) -> Result<isize, i32> {
    sys_write(fd_no, buf, len)
}

fn sys_recvfrom(
    fd_no: usize,
    buf: usize,
    len: usize,
    _flags: usize,
    addr_va: usize,
    len_va: usize,
) -> Result<isize, i32> {
    let n = sys_read(fd_no, buf, len)?;
    if addr_va != 0 && n >= 0 {
        let f = get_fd(fd_no)?;
        if let FdKind::Socket(id) = &f.kind {
            if let Some((ip, port)) = crate::net::getpeername(*id) {
                let _ = write_sockaddr(addr_va, len_va, ip, port);
            }
        }
    }
    Ok(n)
}

fn sys_sendmsg(fd_no: usize, msg_va: usize) -> Result<isize, i32> {
    // msghdr: name(8) namelen(8) iov(8) iovlen(8) control(8) controllen(8) flags(4)
    let iov: usize = uread_val(msg_va + 16)?;
    let iovlen: usize = uread_val(msg_va + 24)?;
    sys_writev(fd_no, iov, iovlen)
}

fn sys_recvmsg(fd_no: usize, msg_va: usize) -> Result<isize, i32> {
    let iov: usize = uread_val(msg_va + 16)?;
    let iovlen: usize = uread_val(msg_va + 24)?;
    sys_readv(fd_no, iov, iovlen)
}

fn sys_setsockopt(
    fd_no: usize,
    level: i32,
    optname: i32,
    optval: usize,
    _optlen: usize,
) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    if level == 6 && optname == 1 {
        // TCP_NODELAY
        let v: i32 = uread_val(optval).unwrap_or(0);
        crate::net::set_nodelay(id, v != 0);
    }
    Ok(0)
}

fn sys_getsockopt(
    fd_no: usize,
    _level: i32,
    _optname: i32,
    optval: usize,
    optlen: usize,
) -> Result<isize, i32> {
    let _f = get_fd(fd_no)?;
    if optval != 0 {
        let _ = uwrite_val(optval, &0i32);
    }
    if optlen != 0 {
        let _ = uwrite_val(optlen, &4u32);
    }
    Ok(0)
}

fn sys_shutdown(fd_no: usize, how: i32) -> Result<isize, i32> {
    let f = get_fd(fd_no)?;
    let id = match &f.kind {
        FdKind::Socket(id) => *id,
        _ => return Err(-ENOTSOCK),
    };
    Ok(crate::net::shutdown(id, how) as isize)
}
