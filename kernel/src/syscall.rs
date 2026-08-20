//! Linux syscall 分发（riscv64）
//!
//! 约定：trap 进入时 satp 仍是用户页表（SUM=1），用户内存可直接解引用，
//! 但必须先用 VMA 检查合法性，避免内核态 page fault。

use alloc::string::String;
use alloc::vec::Vec;
use crate::errno::{ret_i64, Errno, Ret};
use crate::net::socket::{self, FdEntry};
use crate::page::{PTE_R, PTE_U, PTE_W, PTE_X};
use crate::proc;
use crate::trap::TrapFrame;
use crate::vfs::{self, FileData};
use crate::{kprintln, syscall_nr as nr};

/// syscall 追踪开关
const TRACE: bool = false;

pub fn dispatch(frame: &mut TrapFrame, n: usize) -> usize {
    let a = [
        frame.x[10] as usize,
        frame.x[11] as usize,
        frame.x[12] as usize,
        frame.x[13] as usize,
        frame.x[14] as usize,
        frame.x[15] as usize,
    ];
    let ret = dispatch_inner(n, a);
    if TRACE {
        let name = name_of(n);
        let v = ret_i64(ret);
        kprintln!("  [sys] {}({:#x},{:#x},{:#x}) = {}", name, a[0], a[1], a[2], v);
    }
    match ret {
        Ok(v) => v,
        Err(e) => (-e.code()) as usize,
    }
}

fn name_of(n: usize) -> &'static str {
    match n {
        17 => "getcwd",
        24 => "sched_yield",
        25 => "fcntl",
        29 => "ioctl",
        49 => "chdir",
        56 => "openat",
        57 => "close",
        59 => "pipe2",
        61 => "getdents64",
        62 => "lseek",
        63 => "read",
        64 => "write",
        65 => "readv",
        66 => "writev",
        71 => "sendfile",
        78 => "readlinkat",
        79 => "newfstatat",
        80 => "fstat",
        93 => "exit",
        94 => "exit_group",
        96 => "set_tid_address",
        98 => "futex",
        99 => "set_robust_list",
        101 => "nanosleep",
        113 => "clock_gettime",
        129 => "kill",
        144 => "setgid",
        146 => "setuid",
        153 => "times",
        155 => "getpid",
        160 => "uname",
        163 => "getrlimit",
        164 => "setrlimit",
        167 => "prctl",
        172 => "getpid",
        173 => "getppid",
        174 => "getuid",
        175 => "geteuid",
        176 => "getgid",
        177 => "getegid",
        178 => "gettid",
        198 => "socket",
        199 => "socketpair",
        200 => "bind",
        201 => "listen",
        202 => "accept",
        203 => "connect",
        206 => "sendto",
        207 => "recvfrom",
        208 => "setsockopt",
        209 => "getsockopt",
        210 => "shutdown",
        214 => "brk",
        215 => "munmap",
        216 => "mremap",
        220 => "clone",
        221 => "execve",
        222 => "mmap",
        226 => "mprotect",
        260 => "wait4",
        261 => "prlimit64",
        278 => "getrandom",
        291 => "statx",
        _ => "?",
    }
}

// ---------------- 用户内存访问 ----------------

/// 检查 [va, va+len) 是否全部落在可访问的用户 VMA 内
pub fn check_user(va: usize, len: usize, write: bool) -> bool {
    if va == 0 && len > 0 {
        return false;
    }
    if len == 0 {
        return true;
    }
    let proc = proc::current();
    let end = va + len;
    if end < va {
        return false;
    }
    let mut covered_until = va;
    while covered_until < end {
        let page = covered_until & !0xfff;
        let mut found = false;
        for v in proc.vmas.iter() {
            if page + 0xfff > v.start && page < v.end {
                if write && v.flags & PTE_W == 0 {
                    return false;
                }
                if !write && v.flags & PTE_R == 0 && v.flags & PTE_X == 0 {
                    return false;
                }
                found = true;
                covered_until = core::cmp::min(v.end, end);
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

pub fn copy_in(va: usize, len: usize) -> Option<Vec<u8>> {
    if !check_user(va, len, false) {
        return None;
    }
    let mut buf = vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, buf.as_mut_ptr(), len);
    }
    Some(buf)
}

pub fn copy_out(va: usize, bytes: &[u8]) -> bool {
    if !check_user(va, bytes.len(), true) {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), va as *mut u8, bytes.len());
    }
    true
}

pub fn read_user_str(va: usize, max: usize) -> Option<String> {
    if !check_user(va, 1, false) {
        return None;
    }
    let mut out = Vec::new();
    let mut p = va;
    while out.len() < max {
        if !check_user(p, 1, false) {
            return None;
        }
        let b = unsafe { *(p as *const u8) };
        if b == 0 {
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        out.push(b);
        p += 1;
    }
    None
}

pub fn read_user_str_array(va: usize) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut p = va;
    loop {
        let ptr = unsafe { *(p as *const u64) } as usize;
        if ptr == 0 {
            return Some(out);
        }
        out.push(read_user_str(ptr, 4096)?);
        p += 8;
    }
}

/// 填充随机字节（伪随机，demo 用途）
pub fn fill_random(buf: &mut [u8]) {
    let mut state = crate::trap::time_ticks() ^ (buf.as_ptr() as u64);
    for b in buf.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state >> 32) as u8;
    }
}

// ---------------- 各 syscall 实现 ----------------

fn dispatch_inner(n: usize, a: [usize; 6]) -> Ret {
    match n {
        nr::IOCTL => sys_ioctl(a[0], a[1], a[2]),
        nr::OPENAT => sys_openat(a[0], a[1], a[2], a[3]),
        nr::CLOSE => sys_close(a[0]),
        nr::READ => sys_read(a[0], a[1], a[2]),
        nr::WRITE => sys_write(a[0], a[1], a[2]),
        nr::READV => sys_readv(a[0], a[1], a[2]),
        nr::WRITEV => sys_writev(a[0], a[1], a[2]),
        nr::LSEEK => sys_lseek(a[0], a[1] as i64, a[2]),
        nr::FSTAT => sys_fstat(a[0], a[1]),
        nr::NEWFSTATAT => sys_newfstatat(a[0], a[1], a[2], a[3]),
        nr::STATX => sys_statx(a[0], a[1], a[2], a[3], a[4]),
        nr::READLINKAT => sys_readlinkat(a[0], a[1], a[2], a[3]),
        nr::FACCESSAT => sys_faccessat(a[0], a[1], a[2], a[3]),
        nr::GETCWD => sys_getcwd(a[0], a[1]),
        nr::CHDIR => sys_chdir(a[1]),
        nr::UNLINKAT => sys_unlinkat(a[1]),
        nr::MKDIRAT => sys_mkdirat(a[1]),
        nr::RENAMEAT => Ok(0), // 不支持
        nr::FCNTL => sys_fcntl(a[0], a[1], a[2]),
        nr::DUP => sys_dup(a[0]),
        nr::DUP3 => sys_dup3(a[0], a[1]),
        nr::PIPE2 => Err(Errno::Enosys),
        nr::GETDENTS64 => Ok(0), // 空目录枚举
        nr::MMAP => sys_mmap(a[0], a[1], a[2], a[3], a[4], a[5] as u64),
        nr::MUNMAP => sys_munmap(a[0], a[1]),
        nr::MPROTECT => sys_mprotect(a[0], a[1], a[2]),
        nr::MADVISE => Ok(0),
        nr::BRK => sys_brk(a[0]),
        nr::CLONE => Err(Errno::Enosys),
        nr::EXECVE => Err(Errno::Enosys),
        nr::EXIT | nr::EXIT_GROUP => sys_exit_group(a[0] as i64),
        nr::WAIT4 => Err(Errno::Echild),
        nr::SET_TID_ADDRESS => Ok(1),
        nr::SET_ROBUST_LIST => Ok(0),
        nr::RSEQ => Err(Errno::Enosys),
        nr::FUTEX => Ok(0),
        nr::NANOSLEEP => sys_nanosleep(a[0], a[1]),
        nr::CLOCK_GETTIME => sys_clock_gettime(a[0], a[1]),
        nr::CLOCK_GETRES => sys_clock_getres(a[0], a[1]),
        nr::GETTIMEOFDAY => sys_gettimeofday(a[0]),
        nr::TIMES => sys_times(a[0]),
        nr::UNAME => sys_uname(a[0]),
        nr::GETRLIMIT => sys_getrlimit(a[0], a[1]),
        nr::SETRLIMIT => Ok(0),
        nr::PRLIMIT64 => sys_prlimit64(a[1], a[2], a[3]),
        nr::GETRANDOM => sys_getrandom(a[0], a[1], a[2]),
        nr::GETPID => Ok(1),
        nr::GETPPID => Ok(0),
        nr::GETTID => Ok(1),
        nr::GETUID | nr::GETEUID => Ok(0),
        nr::GETGID | nr::GETEGID => Ok(0),
        nr::SCHED_YIELD => Ok(0),
        nr::KILL | nr::TGKILL => Ok(0),
        nr::RT_SIGACTION => sys_rt_sigaction(a[0], a[1], a[2]),
        nr::RT_SIGPROCMASK => sys_rt_sigprocmask(a[0], a[1], a[2]),
        nr::RT_SIGRETURN => Ok(0),
        nr::SIGALTSTACK => Ok(0),
        nr::PRCTL => Ok(0),
        nr::SETUID | nr::SETGID => Ok(0),
        nr::SETGROUPS => Ok(0),
        nr::UMASK => Ok(0o22),
        nr::GETPGID => Ok(1),
        nr::SETSID => Ok(1),
        nr::SOCKET => socket::sys_socket(a[0], a[1], a[2]),
        nr::SOCKETPAIR => Err(Errno::Enosys),
        nr::BIND => sys_bind(a[0], a[1], a[2]),
        nr::LISTEN => socket::sys_listen(a[0], a[1]),
        nr::ACCEPT | nr::ACCEPT4 => sys_accept4(a[0], a[1], a[2], a[3]),
        nr::CONNECT => sys_connect(a[0], a[1], a[2]),
        nr::SENDTO => sys_sendto(a[0], a[1], a[2], a[3]),
        nr::RECVFROM => sys_recvfrom(a[0], a[1], a[2]),
        nr::SETSOCKOPT => sys_setsockopt(a[0], a[1], a[2], a[3]),
        nr::GETSOCKOPT => sys_getsockopt(a[0], a[1], a[2], a[3]),
        nr::SHUTDOWN => sys_shutdown(a[0], a[1]),
        nr::SENDFILE => sys_sendfile(a[0], a[1], a[2], a[3]),
        nr::EPOLL_CREATE1 => socket::sys_epoll_create1(),
        nr::EPOLL_CTL => sys_epoll_ctl(a[0], a[1], a[2], a[3]),
        nr::EPOLL_PWAIT | nr::EPOLL_WAIT => sys_epoll_wait(a[0], a[1], a[2], a[3] as i64),
        _ => {
            kprintln!("[sys] unimplemented syscall {} ({:#x})", n, n);
            Err(Errno::Enosys)
        }
    }
}

// ---------------- 文件 ----------------

const AT_FDCWD: usize = (-100i64) as usize;

fn resolve_fd_path(dirfd: usize, path_ptr: usize) -> Result<String, Errno> {
    let path = read_user_str(path_ptr, 4096).ok_or(Errno::Efault)?;
    if path.starts_with('/') {
        Ok(path)
    } else {
        let base = if dirfd == AT_FDCWD {
            proc::current().cwd.clone()
        } else {
            String::from("/")
        };
        Ok(format!("{}/{}", base, path))
    }
}

const O_ACCMODE: usize = 3;
const O_CREAT: usize = 0x40;
const O_TRUNC: usize = 0x200;
const O_APPEND: usize = 0x400;
const O_DIRECTORY: usize = 0x10000;

fn sys_openat(_dirfd: usize, path_ptr: usize, flags: usize, _mode: usize) -> Ret {
    let path = resolve_fd_path(_dirfd, path_ptr)?;
    let acc = flags & O_ACCMODE;
    let want_dir = flags & O_DIRECTORY != 0;

    // 特殊设备
    if path == "/dev/null" {
        let fd = proc::alloc_fd();
        proc::current().fds[fd] = Some(FdEntry::Null);
        return Ok(fd);
    }
    if path == "/dev/stdin" || path == "/dev/stdout" || path == "/dev/stderr" {
        let fd = proc::alloc_fd();
        proc::current().fds[fd] = Some(FdEntry::Console);
        return Ok(fd);
    }

    let meta = vfs::stat_path(&path);
    if want_dir {
        if meta.exists && meta.is_dir {
            let fd = proc::alloc_fd();
            // 目录：空文件句柄（getdents 返回空）
            proc::current().fds[fd] = Some(FdEntry::File {
                data: FileData::Static(&[]),
                pos: 0,
                append: false,
            });
            return Ok(fd);
        }
        return Err(Errno::Enoent);
    }

    if !meta.exists {
        if flags & O_CREAT != 0 {
            // tmpfs 创建
            let f = vfs::create_write(&path, flags & O_TRUNC != 0);
            let fd = proc::alloc_fd();
            proc::current().fds[fd] = Some(FdEntry::File {
                data: FileData::Tmp(f),
                pos: 0,
                append: flags & O_APPEND != 0,
            });
            return Ok(fd);
        }
        return Err(Errno::Enoent);
    }

    // 存在：读打开（或写打开 tmpfs）
    match vfs::open_read(&path) {
        Some((FileData::Static(b), mode)) => {
            if acc != 0 {
                // 写 rootfs → 复制到 tmpfs 再写
                if flags & O_CREAT != 0 && flags & O_TRUNC != 0 {
                    // 截断写：重定向到 tmpfs 空文件
                    let f = vfs::create_write(&path, true);
                    let fd = proc::alloc_fd();
                    proc::current().fds[fd] = Some(FdEntry::File {
                        data: FileData::Tmp(f),
                        pos: 0,
                        append: flags & O_APPEND != 0,
                    });
                    return Ok(fd);
                }
                if flags & O_APPEND != 0 {
                    // 追加：复制到 tmpfs
                    let f = vfs::create_write(&path, false);
                    f.borrow_mut().extend_from_slice(b);
                    let fd = proc::alloc_fd();
                    let pos = b.len();
                    proc::current().fds[fd] = Some(FdEntry::File {
                        data: FileData::Tmp(f),
                        pos,
                        append: true,
                    });
                    return Ok(fd);
                }
                return Err(Errno::Erofs);
            }
            let fd = proc::alloc_fd();
            proc::current().fds[fd] = Some(FdEntry::File {
                data: FileData::Static(b),
                pos: 0,
                append: false,
            });
            Ok(fd)
        }
        Some((FileData::Tmp(v), _)) => {
            if acc != 0 && flags & O_TRUNC != 0 {
                v.borrow_mut().clear();
            }
            let pos = if flags & O_APPEND != 0 { v.borrow().len() } else { 0 };
            let fd = proc::alloc_fd();
            proc::current().fds[fd] = Some(FdEntry::File {
                data: FileData::Tmp(v),
                pos,
                append: flags & O_APPEND != 0,
            });
            Ok(fd)
        }
        None => {
            if meta.is_dir {
                let fd = proc::alloc_fd();
                proc::current().fds[fd] = Some(FdEntry::File {
                    data: FileData::Static(&[]),
                    pos: 0,
                    append: false,
                });
                return Ok(fd);
            }
            Err(Errno::Eacces)
        }
    }
}

fn sys_close(fd: usize) -> Ret {
    socket::sys_close_fd(fd)?;
    proc::current().fds[fd] = None;
    Ok(0)
}

fn fd_read_bytes(fd: usize, buf: &mut [u8]) -> Ret {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Console => {
            // 控制台读：非阻塞，无输入返回 EAGAIN
            match crate::uart::get_byte() {
                Some(b) => {
                    buf[0] = b;
                    Ok(1)
                }
                None => Err(Errno::Eagain),
            }
        }
        FdEntry::Null => Ok(0),
        FdEntry::File { data, pos, .. } => match data {
            FileData::Static(b) => {
                let start = *pos;
                if start >= b.len() {
                    return Ok(0);
                }
                let n = core::cmp::min(buf.len(), b.len() - start);
                buf[..n].copy_from_slice(&b[start..start + n]);
                *pos += n;
                Ok(n)
            }
            FileData::Tmp(v) => {
                let v = v.borrow();
                let start = *pos;
                if start >= v.len() {
                    return Ok(0);
                }
                let n = core::cmp::min(buf.len(), v.len() - start);
                buf[..n].copy_from_slice(&v[start..start + n]);
                *pos += n;
                Ok(n)
            }
        },
        FdEntry::Socket(_) => socket::sys_recv(fd, buf, 0),
        FdEntry::Epoll(_) => Err(Errno::Einval),
    }
}

fn fd_write_bytes(fd: usize, bytes: &[u8]) -> Ret {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Console => {
            crate::uart::write_bytes(bytes);
            Ok(bytes.len())
        }
        FdEntry::Null => Ok(bytes.len()),
        FdEntry::File { data, pos, append } => {
            match data {
                FileData::Static(_) => Err(Errno::Erofs), // rootfs 只读
                FileData::Tmp(v) => {
                    let mut v = v.borrow_mut();
                    if *append {
                        v.extend_from_slice(bytes);
                        *pos = v.len();
                    } else {
                        if *pos > v.len() {
                            v.resize(*pos, 0);
                        }
                        let end = *pos + bytes.len();
                        if end > v.len() {
                            v.resize(end, 0);
                        }
                        v[*pos..end].copy_from_slice(bytes);
                        *pos = end;
                    }
                    Ok(bytes.len())
                }
            }
        }
        FdEntry::Socket(_) => socket::sys_send(fd, bytes, 0),
        FdEntry::Epoll(_) => Err(Errno::Einval),
    }
}

fn sys_read(fd: usize, buf_va: usize, len: usize) -> Ret {
    if !check_user(buf_va, len, true) {
        return Err(Errno::Efault);
    }
    let mut buf = vec![0u8; len];
    let n = fd_read_bytes(fd, &mut buf)?;
    if n > 0 && !copy_out(buf_va, &buf[..n]) {
        return Err(Errno::Efault);
    }
    Ok(n)
}

fn sys_write(fd: usize, buf_va: usize, len: usize) -> Ret {
    let buf = copy_in(buf_va, len).ok_or(Errno::Efault)?;
    fd_write_bytes(fd, &buf)
}

// iovec: { base: u64, len: u64 }
fn sys_readv(fd: usize, iov_va: usize, iovcnt: usize) -> Ret {
    if iovcnt > 1024 {
        return Err(Errno::Einval);
    }
    let iovs = copy_in(iov_va, iovcnt * 16).ok_or(Errno::Efault)?;
    let mut total = 0usize;
    for i in 0..iovcnt {
        let base = u64::from_le_bytes(iovs[i * 16..i * 16 + 8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(iovs[i * 16 + 8..i * 16 + 16].try_into().unwrap()) as usize;
        if len == 0 {
            continue;
        }
        let r = sys_read(fd, base, len);
        match r {
            Ok(n) => {
                total += n;
                if n < len {
                    break;
                }
            }
            Err(Errno::Eagain) => {
                if total > 0 {
                    break;
                }
                return Err(Errno::Eagain);
            }
            Err(e) => {
                if total > 0 {
                    break;
                }
                return Err(e);
            }
        }
    }
    Ok(total)
}

fn sys_writev(fd: usize, iov_va: usize, iovcnt: usize) -> Ret {
    if iovcnt > 1024 {
        return Err(Errno::Einval);
    }
    let iovs = copy_in(iov_va, iovcnt * 16).ok_or(Errno::Efault)?;
    let mut total = 0usize;
    for i in 0..iovcnt {
        let base = u64::from_le_bytes(iovs[i * 16..i * 16 + 8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(iovs[i * 16 + 8..i * 16 + 16].try_into().unwrap()) as usize;
        if len == 0 {
            continue;
        }
        let buf = match copy_in(base, len) {
            Some(b) => b,
            None => return Err(Errno::Efault),
        };
        match fd_write_bytes(fd, &buf) {
            Ok(n) => total += n,
            Err(e) => {
                if total > 0 {
                    break;
                }
                return Err(e);
            }
        }
    }
    Ok(total)
}

fn sys_lseek(fd: usize, off: i64, whence: usize) -> Ret {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::File { data, pos, .. } => {
            let len = match data {
                FileData::Static(b) => b.len(),
                FileData::Tmp(v) => v.borrow().len(),
            };
            let new: i64 = match whence {
                0 => off,
                1 => *pos as i64 + off,
                2 => len as i64 + off,
                _ => return Err(Errno::Einval),
            };
            if new < 0 {
                return Err(Errno::Einval);
            }
            *pos = new as usize;
            Ok(*pos)
        }
        _ => Err(Errno::EspiPE),
    }
}

// ---------------- stat 系列 ----------------

fn stat_meta_to_bytes(meta: &vfs::Meta, st: &mut [u8]) {
    st.fill(0);
    let mode = if meta.is_dir {
        0o040000 | (meta.mode & 0o777)
    } else if meta.is_symlink {
        0o120000 | 0o777
    } else {
        0o100000 | (meta.mode & 0o777)
    };
    st[0..8].copy_from_slice(&0x801u64.to_le_bytes()); // dev
    st[8..16].copy_from_slice(&1u64.to_le_bytes()); // ino
    st[16..20].copy_from_slice(&mode.to_le_bytes());
    st[20..24].copy_from_slice(&1u32.to_le_bytes()); // nlink
    st[24..28].copy_from_slice(&0u32.to_le_bytes()); // uid
    st[28..32].copy_from_slice(&0u32.to_le_bytes()); // gid
    st[32..40].copy_from_slice(&0x801u64.to_le_bytes()); // rdev
    st[48..56].copy_from_slice(&(meta.size as u64).to_le_bytes());
    st[56..60].copy_from_slice(&4096u32.to_le_bytes()); // blksize
    st[64..72].copy_from_slice(&((meta.size as u64 + 511) / 512).to_le_bytes()); // blocks
    let now = crate::trap::now_ms();
    st[72..80].copy_from_slice(&((now / 1000) as u64).to_le_bytes());
    st[80..88].copy_from_slice(&((now % 1000) * 1_000_000).to_le_bytes());
    st[88..96].copy_from_slice(&((now / 1000) as u64).to_le_bytes());
    st[96..104].copy_from_slice(&((now % 1000) * 1_000_000).to_le_bytes());
    st[104..112].copy_from_slice(&((now / 1000) as u64).to_le_bytes());
    st[112..120].copy_from_slice(&((now % 1000) * 1_000_000).to_le_bytes());
}

fn fd_meta(fd: usize) -> Result<vfs::Meta, Errno> {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::File { data, .. } => {
            let (size, mode) = match data {
                FileData::Static(b) => (b.len(), 0o644),
                FileData::Tmp(v) => (v.borrow().len(), 0o644),
            };
            Ok(vfs::Meta {
                exists: true,
                is_dir: false,
                is_symlink: false,
                size,
                mode,
            })
        }
        FdEntry::Console | FdEntry::Null => Ok(vfs::Meta {
            exists: true,
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0o620,
        }),
        FdEntry::Socket(_) => Ok(vfs::Meta {
            exists: true,
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0o140777,
        }),
        FdEntry::Epoll(_) => Ok(vfs::Meta {
            exists: true,
            is_dir: false,
            is_symlink: false,
            size: 0,
            mode: 0o100600,
        }),
    }
}

fn sys_fstat(fd: usize, st_va: usize) -> Ret {
    let meta = fd_meta(fd)?;
    if !check_user(st_va, 128, true) {
        return Err(Errno::Efault);
    }
    let mut buf = [0u8; 128];
    stat_meta_to_bytes(&meta, &mut buf);
    if !copy_out(st_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_newfstatat(_dirfd: usize, path_va: usize, st_va: usize, flags: usize) -> Ret {
    let path = resolve_fd_path(_dirfd, path_va)?;
    let meta = vfs::stat_path(&path);
    if !meta.exists {
        // AT_EMPTY_PATH / AT_SYMLINK_NOFOLLOW 时 nginx 用 fstat 路径
        if flags & 0x1000 != 0 {
            // AT_SYMLINK_NOFOLLOW: 也视为不存在
            return Err(Errno::Enoent);
        }
        return Err(Errno::Enoent);
    }
    let mut buf = [0u8; 128];
    stat_meta_to_bytes(&meta, &mut buf);
    if !copy_out(st_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_statx(_dirfd: usize, path_va: usize, _flags: usize, mask: usize, buf_va: usize) -> Ret {
    let path = resolve_fd_path(_dirfd, path_va)?;
    let meta = vfs::stat_path(&path);
    if !meta.exists {
        return Err(Errno::Enoent);
    }
    let mut buf = [0u8; 256];
    // struct statx 布局
    buf[0..4].copy_from_slice(&mask.to_le_bytes());
    buf[16..20].copy_from_slice(&(meta.mode | if meta.is_dir { 0o040000 } else { 0o100000 }).to_le_bytes());
    buf[32..40].copy_from_slice(&(meta.size as u64).to_le_bytes());
    if !copy_out(buf_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_readlinkat(_dirfd: usize, path_va: usize, buf_va: usize, size: usize) -> Ret {
    let path = resolve_fd_path(_dirfd, path_va)?;
    // /proc/self/exe → nginx 路径
    let target: &[u8] = if path.contains("self/exe") {
        b"/usr/sbin/nginx"
    } else if path.contains("self/fd/") {
        b"/usr/sbin/nginx"
    } else if path == "/proc/mounts" || path.contains("self/mounts") {
        b"/dev/root"
    } else {
        b""
    };
    let n = core::cmp::min(target.len(), size);
    if !copy_out(buf_va, &target[..n]) {
        return Err(Errno::Efault);
    }
    Ok(n)
}

fn sys_faccessat(_dirfd: usize, path_va: usize, _mode: usize, _flags: usize) -> Ret {
    let path = resolve_fd_path(_dirfd, path_va)?;
    let meta = vfs::stat_path(&path);
    if meta.exists {
        Ok(0)
    } else {
        Err(Errno::Enoent)
    }
}

fn sys_getcwd(buf_va: usize, size: usize) -> Ret {
    let cwd = proc::current().cwd.clone();
    let out = format!("/{}\0", cwd);
    if out.len() > size {
        return Err(Errno::Erange);
    }
    if !copy_out(buf_va, out.as_bytes()) {
        return Err(Errno::Efault);
    }
    Ok(out.len() - 1)
}

fn sys_chdir(path_va: usize) -> Ret {
    let path = read_user_str(path_va, 4096).ok_or(Errno::Efault)?;
    let meta = vfs::stat_path(&path);
    if !meta.exists {
        return Err(Errno::Enoent);
    }
    if !meta.is_dir {
        return Err(Errno::Enotdir);
    }
    proc::current().cwd = vfs::normalize_path(&path);
    Ok(0)
}

fn sys_unlinkat(path_va: usize) -> Ret {
    let path = read_user_str(path_va, 4096).ok_or(Errno::Efault)?;
    if vfs::unlink(&path) {
        Ok(0)
    } else {
        Err(Errno::Enoent)
    }
}

fn sys_mkdirat(path_va: usize) -> Ret {
    let path = read_user_str(path_va, 4096).ok_or(Errno::Efault)?;
    vfs::mkdir(&path);
    Ok(0)
}

// ---------------- 内存 ----------------

const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

fn sys_mmap(addr: usize, len: usize, prot: usize, flags: usize, fd: usize, off: u64) -> Ret {
    if len == 0 {
        return Err(Errno::Einval);
    }
    if len > 256 << 20 {
        return Err(Errno::Enomem);
    }
    let pte_flags = {
        let mut f = PTE_U;
        if prot & 1 != 0 {
            f |= PTE_R;
        }
        if prot & 2 != 0 {
            f |= PTE_W;
        }
        if prot & 4 != 0 {
            f |= PTE_X;
        }
        f
    };

    let proc = proc::current();
    // 选择映射地址
    let start = if flags & MAP_FIXED != 0 && addr != 0 {
        addr & !0xfff
    } else if addr != 0 && !proc.vmas.iter().any(|v| addr + len > v.start && addr < v.end) {
        addr & !0xfff
    } else {
        let s = proc.mmap_next;
        proc.mmap_next = (s + len + 0xfff) & !0xfff;
        s
    };

    if !proc::add_vma(start, start + len, pte_flags) {
        return Err(Errno::Enomem);
    }

    let root = proc::current_page_table_root();

    if flags & MAP_ANONYMOUS != 0 {
        // 匿名：lazy 零页（page fault 分配）
        return Ok(start);
    }

    // 文件映射：eager 复制
    let data = {
        let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
        match entry {
            FdEntry::File { data, .. } => match data {
                FileData::Static(b) => b.to_vec(),
                FileData::Tmp(v) => v.borrow().clone(),
            },
            _ => return Err(Errno::Eacces),
        }
    };
    let off = off as usize;
    // 填充映射内容
    let mut va = start;
    while va < start + len {
        let pa = crate::pmm::alloc_page().ok_or(Errno::Enomem)?;
        crate::page::map_4k(root, va, pa, pte_flags | crate::page::PTE_A | crate::page::PTE_D | crate::page::PTE_V);
        // 复制文件数据
        let file_pos = off + (va - start);
        if file_pos < data.len() {
            let n = core::cmp::min(0x1000, data.len() - file_pos);
            unsafe {
                core::ptr::copy_nonoverlapping(data[file_pos..file_pos + n].as_ptr(), pa as *mut u8, n);
            }
        }
        va += 0x1000;
    }
    Ok(start)
}

fn sys_munmap(addr: usize, len: usize) -> Ret {
    let start = addr & !0xfff;
    let end = (addr + len + 0xfff) & !0xfff;
    let proc = proc::current();
    let root = proc.root;
    let mut va = start;
    while va < end {
        crate::page::unmap(root, va);
        va += 0x1000;
    }
    proc.vmas.retain(|v| v.end <= start || v.start >= end);
    Ok(0)
}

fn sys_mprotect(addr: usize, len: usize, prot: usize) -> Ret {
    let start = addr & !0xfff;
    let end = (addr + len + 0xfff) & !0xfff;
    let mut f = PTE_U;
    if prot & 1 != 0 {
        f |= PTE_R;
    }
    if prot & 2 != 0 {
        f |= PTE_W;
    }
    if prot & 4 != 0 {
        f |= PTE_X;
    }
    let proc = proc::current();
    let root = proc.root;
    let mut va = start;
    while va < end {
        crate::page::remap_flags(root, va, f | crate::page::PTE_A | crate::page::PTE_D);
        va += 0x1000;
    }
    for v in proc.vmas.iter_mut() {
        if v.start < end && v.end > start {
            v.flags = f;
        }
    }
    Ok(0)
}

fn sys_brk(new: usize) -> Ret {
    let proc = proc::current();
    if new == 0 {
        return Ok(proc.brk);
    }
    if new < proc.brk {
        // 缩小：简单忽略
        return Ok(proc.brk);
    }
    if new > proc.brk {
        // 扩展 brk VMA
        let old = proc.brk;
        // 检查不与 mmap 区冲突
        if proc.vmas.iter().any(|v| new > v.start && old < v.end) {
            return Ok(proc.brk);
        }
        // 合并/添加 VMA
        let mut merged = false;
        for v in proc.vmas.iter_mut() {
            if v.end == old && v.flags & PTE_U != 0 {
                v.end = (new + 0xfff) & !0xfff;
                merged = true;
                break;
            }
        }
        if !merged {
            proc.vmas.push(proc::Vma {
                start: old,
                end: (new + 0xfff) & !0xfff,
                flags: PTE_U | PTE_R | PTE_W,
            });
        }
        proc.brk = (new + 0xfff) & !0xfff;
    }
    Ok(proc.brk)
}

// ---------------- 进程/杂项 ----------------

fn sys_exit_group(code: i64) -> Ret {
    kprintln!("\n[user] exit_group({})", code);
    crate::net::poll_flush();
    crate::sbi::shutdown()
}

fn sys_rt_sigaction(_sig: usize, _act: usize, _old: usize) -> Ret {
    Ok(0)
}

fn sys_rt_sigprocmask(_how: usize, _set: usize, _old: usize) -> Ret {
    Ok(0)
}

fn sys_nanosleep(req_va: usize, _rem: usize) -> Ret {
    let ts = copy_in(req_va, 16).ok_or(Errno::Efault)?;
    let sec = u64::from_le_bytes(ts[0..8].try_into().unwrap());
    let nsec = u64::from_le_bytes(ts[8..16].try_into().unwrap());
    let mut ms = sec * 1000 + nsec / 1_000_000;
    if ms == 0 {
        ms = 1;
    }
    let mut left = ms;
    while left > 0 {
        let step = core::cmp::min(left, 100);
        crate::net::stack::wait_ms(step);
        left -= step;
    }
    Ok(0)
}

fn sys_clock_gettime(clk: usize, tp_va: usize) -> Ret {
    let ms = match clk {
        0 => {
            // CLOCK_REALTIME：从 2026-01-01 起的伪真实时钟
            1767225600_000 + crate::trap::now_ms()
        }
        _ => crate::trap::now_ms(),
    };
    let sec = ms / 1000;
    let nsec = (ms % 1000) * 1_000_000;
    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&sec.to_le_bytes());
    buf[8..16].copy_from_slice(&nsec.to_le_bytes());
    if !copy_out(tp_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_clock_getres(_clk: usize, res_va: usize) -> Ret {
    let mut buf = [0u8; 16];
    buf[8..16].copy_from_slice(&1u64.to_le_bytes()); // 1ns
    if !copy_out(res_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_gettimeofday(tv_va: usize) -> Ret {
    let ms = 1767225600_000 + crate::trap::now_ms();
    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    buf[8..16].copy_from_slice(&((ms % 1000) * 1000).to_le_bytes());
    if !copy_out(tv_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_times(_buf_va: usize) -> Ret {
    Ok(0)
}

fn sys_uname(buf_va: usize) -> Ret {
    // struct utsname: 6 × 65 字节
    let mut buf = [0u8; 390];
    let fields = [
        "Linux",
        "ijiege",
        "6.1.0-ijiege",
        "#1 SMP",
        "riscv64",
        "",
    ];
    let mut off = 0usize;
    for f in fields.iter() {
        let b = f.as_bytes();
        buf[off..off + b.len()].copy_from_slice(b);
        off += 65;
    }
    if !copy_out(buf_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_getrlimit(res: usize, rlim_va: usize) -> Ret {
    let mut buf = [0u8; 16];
    let (cur, max) = match res {
        3 => (8 << 20, 8 << 20),       // RLIMIT_STACK
        7 => (65536, 65536),           // RLIMIT_NOFILE
        4 => (0, 0),                   // RLIMIT_CORE
        _ => (usize::MAX, usize::MAX),
    };
    buf[0..8].copy_from_slice(&cur.to_le_bytes());
    buf[8..16].copy_from_slice(&max.to_le_bytes());
    if !copy_out(rlim_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(0)
}

fn sys_prlimit64(_pid: usize, res: usize, new_va: usize) -> Ret {
    if new_va != 0 {
        // 读取新值（只允许调 nofile）
        if let Some(new) = copy_in(new_va, 16) {
            let _cur = u64::from_le_bytes(new[0..8].try_into().unwrap());
            let _max = u64::from_le_bytes(new[8..16].try_into().unwrap());
            let _ = res;
        }
    }
    Ok(0)
}

fn sys_getrandom(buf_va: usize, len: usize, _flags: usize) -> Ret {
    if !check_user(buf_va, len, true) {
        return Err(Errno::Efault);
    }
    let mut buf = vec![0u8; len];
    fill_random(&mut buf);
    if !copy_out(buf_va, &buf) {
        return Err(Errno::Efault);
    }
    Ok(len)
}

fn sys_fcntl(fd: usize, cmd: usize, arg: usize) -> Ret {
    match cmd {
        1 | 2 => {
            // F_DUPFD / F_DUPFD_CLOEXEC
            let newfd = proc::alloc_fd();
            if newfd < arg {
                proc::current().fds[newfd] = None;
                return Err(Errno::Einval);
            }
            let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
            proc::current().fds[newfd] = Some(entry.clone());
            Ok(newfd)
        }
        3 => {
            // F_GETFL
            let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
            let flags = match entry {
                FdEntry::Socket(s) => 0x2 | if s.borrow().nonblocking { 0x800 } else { 0 }, // O_RDWR | O_NONBLOCK
                _ => 0x2,
            };
            Ok(flags)
        }
        4 => {
            // F_SETFL：只关心 O_NONBLOCK
            const O_NONBLOCK: usize = 0x800;
            let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
            if let FdEntry::Socket(s) = entry {
                s.borrow_mut().nonblocking = arg & O_NONBLOCK != 0;
            }
            Ok(0)
        }
        8 => Ok(0),  // F_GETFD
        9 => Ok(0),  // F_SETFD
        1030 => Ok(0), // F_ADD_SEALS?
        _ => {
            kprintln!("[sys] fcntl({},{:#x},{:#x}) unsupported", fd, cmd, arg);
            Ok(0)
        }
    }
}

fn sys_dup(fd: usize) -> Ret {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    let newfd = proc::alloc_fd();
    proc::current().fds[newfd] = Some(entry.clone());
    Ok(newfd)
}

fn sys_dup3(fd: usize, newfd: usize) -> Ret {
    let entry = proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    let proc = proc::current();
    while proc.fds.len() <= newfd {
        proc.fds.push(None);
    }
    proc.fds[newfd] = Some(entry.clone());
    Ok(newfd)
}

// ---------------- socket ----------------

fn sys_bind(fd: usize, addr_va: usize, _len: usize) -> Ret {
    let addr = copy_in(addr_va, 16).ok_or(Errno::Efault)?;
    socket::sys_bind(fd, &addr)
}

fn sys_connect(fd: usize, addr_va: usize, _len: usize) -> Ret {
    let addr = copy_in(addr_va, 16).ok_or(Errno::Efault)?;
    socket::sys_connect(fd, &addr)
}

fn sys_accept4(fd: usize, addr_va: usize, addrlen_va: usize, flags: usize) -> Ret {
    let nonblock = flags & 0x800 != 0; // SOCK_NONBLOCK
    // 输出地址可选
    let mut addr_buf = [0u8; 16];
    let has_addr = addr_va != 0;
    let r = socket::sys_accept4(fd, if has_addr { Some(&mut addr_buf[..]) } else { None }, nonblock);
    if r.is_ok() && has_addr {
        copy_out(addr_va, &addr_buf);
        if addrlen_va != 0 {
            let len = 16u32;
            copy_out(addrlen_va, &len.to_le_bytes());
        }
    }
    r
}

fn sys_sendto(fd: usize, buf_va: usize, len: usize, _flags: usize) -> Ret {
    let buf = copy_in(buf_va, len).ok_or(Errno::Efault)?;
    socket::sys_send(fd, &buf, _flags)
}

fn sys_recvfrom(fd: usize, buf_va: usize, len: usize, _flags: usize) -> Ret {
    let mut buf = vec![0u8; len];
    let n = socket::sys_recv(fd, &mut buf, _flags)?;
    if n > 0 && !copy_out(buf_va, &buf[..n]) {
        return Err(Errno::Efault);
    }
    Ok(n)
}

fn sys_setsockopt(fd: usize, level: usize, opt: usize, val_va: usize) -> Ret {
    let val = copy_in(val_va, 8).ok_or(Errno::Efault)?;
    socket::sys_setsockopt(fd, level, opt, &val)
}

fn sys_getsockopt(fd: usize, level: usize, opt: usize, val_va: usize) -> Ret {
    let mut val = [0u8; 8];
    let r = socket::sys_getsockopt(fd, level, opt, &mut val);
    if r.is_ok() {
        copy_out(val_va, &val[..4]);
    }
    r
}

fn sys_shutdown(fd: usize, how: usize) -> Ret {
    socket::sys_shutdown(fd, how)
}

fn sys_sendfile(out_fd: usize, in_fd: usize, off_va: usize, count: usize) -> Ret {
    if off_va != 0 {
        // 有 offset 指针：使用它作为起始（读后更新）
        let off = copy_in(off_va, 8).ok_or(Errno::Efault)?;
        let start = u64::from_le_bytes(off[0..8].try_into().unwrap()) as usize;
        // 手动 seek
        let entry = proc::get_fd(in_fd).ok_or(Errno::Ebadf)?;
        if let FdEntry::File { pos, .. } = entry {
            *pos = start;
        }
        let r = socket::sys_sendfile(out_fd, in_fd, count);
        if let Ok(n) = r {
            let entry = proc::get_fd(in_fd).unwrap();
            if let FdEntry::File { pos, .. } = entry {
                copy_out(off_va, &(*pos as u64).to_le_bytes());
            }
        }
        r
    } else {
        socket::sys_sendfile(out_fd, in_fd, count)
    }
}

fn sys_epoll_ctl(epfd: usize, op: usize, fd: usize, ev_va: usize) -> Ret {
    let mut events = 0u32;
    let mut data = 0u64;
    if op != socket::EPOLL_CTL_DEL {
        let ev = copy_in(ev_va, 12).ok_or(Errno::Efault)?;
        events = u32::from_le_bytes(ev[0..4].try_into().unwrap());
        data = u64::from_le_bytes(ev[4..12].try_into().unwrap());
    }
    socket::sys_epoll_ctl(epfd, op, fd, events, data)
}

fn sys_epoll_wait(epfd: usize, ev_va: usize, maxevents: usize, timeout: i64) -> Ret {
    if maxevents == 0 || maxevents > 4096 {
        return Err(Errno::Einval);
    }
    let mut events = vec![(0u32, 0u64); maxevents];
    let n = socket::sys_epoll_wait(epfd, &mut events, timeout)?;
    // epoll_event: { events: u32, data: u64 }
    let mut out = vec![0u8; maxevents * 12];
    for (i, (e, d)) in events.iter().enumerate().take(n) {
        out[i * 12..i * 12 + 4].copy_from_slice(&e.to_le_bytes());
        out[i * 12 + 4..i * 12 + 12].copy_from_slice(&d.to_le_bytes());
    }
    if n > 0 && !copy_out(ev_va, &out[..n * 12]) {
        return Err(Errno::Efault);
    }
    Ok(n)
}
