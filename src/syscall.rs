//! Linux RISC-V 系统调用模拟（nginx 所需子集）。
//!
//! 调用约定：a7=系统调用号，a0..a5 参数，返回值放 a0。
//! 负值表示 -errno。

use crate::trap::TrapContext;
use crate::sched::current_process;
use crate::mm::PAGE_SIZE;

// errno
const EBADF: isize = -9;
const ENOSYS: isize = -38;
const ENOENT: isize = -2;
const EINVAL: isize = -22;
const EFAULT: isize = -14;

// syscall numbers (Linux RISC-V)
const SYS_read: usize = 63;
const SYS_write: usize = 64;
const SYS_openat: usize = 56;
const SYS_close: usize = 57;
const SYS_fstat: usize = 80;
const SYS_newfstatat: usize = 79;
const SYS_stat: usize = 1062;
const SYS_lseek: usize = 62;
const SYS_exit: usize = 93;
const SYS_exit_group: usize = 94;
const SYS_kill: usize = 129;
const SYS_uname: usize = 160;
const SYS_getpid: usize = 172;
const SYS_getuid: usize = 174;
const SYS_geteuid: usize = 175;
const SYS_getgid: usize = 176;
const SYS_getegid: usize = 177;
const SYS_gettid: usize = 178;
const SYS_set_tid_address: usize = 96;
const SYS_set_robust_list: usize = 99;
const SYS_futex: usize = 98;
const SYS_nanosleep: usize = 101;
const SYS_clock_gettime: usize = 113;
const SYS_brk: usize = 214;
const SYS_munmap: usize = 215;
const SYS_mmap: usize = 222;
const SYS_mprotect: usize = 226;
const SYS_prlimit64: usize = 261;
const SYS_getrandom: usize = 278;
const SYS_rt_sigaction: usize = 134;
const SYS_rt_sigprocmask: usize = 135;
const SYS_ioctl: usize = 29;
const SYS_clone: usize = 220;
const SYS_execve: usize = 221;
const SYS_clone3: usize = 435;
// epoll / eventfd
const SYS_epoll_create1: usize = 20;
const SYS_epoll_ctl: usize = 21;
const SYS_epoll_pwait: usize = 22;
const SYS_eventfd2: usize = 19;
const SYS_pipe2: usize = 59;
const SYS_dup3: usize = 24;
const SYS_madvise: usize = 233;
const SYS_getrlimit: usize = 163;
const SYS_setrlimit: usize = 164;
const SYS_getrusage: usize = 165;
const SYS_sched_yield: usize = 124;
const SYS_sigaltstack: usize = 132;
const SYS_rt_sigsuspend: usize = 133;
const SYS_rt_sigreturn: usize = 139;
const SYS_setsid: usize = 157;
const SYS_setgid: usize = 144;
const SYS_setuid: usize = 146;
const SYS_setpgid: usize = 154;
const SYS_chdir: usize = 49;
const SYS_chroot: usize = 51;
const SYS_umask: usize = 166;
const SYS_mkdirat: usize = 34;
const SYS_poll: usize = 7;
const SYS_pread64: usize = 67;
const SYS_pwrite64: usize = 68;
const SYS_socketpair: usize = 199;
const SYS_sched_getaffinity: usize = 123;
const SYS_getppid: usize = 173;
const SYS_sysinfo: usize = 179;
const SYS_prctl: usize = 167;
const SYS_unshare: usize = 97;
const SYS_set_tid_address_2: usize = 96;
const SYS_getcwd: usize = 17;
const SYS_readlinkat: usize = 78;
const SYS_readlink: usize = 89;
const SYS_writev: usize = 66;
const SYS_ppoll: usize = 73;
const SYS_pselect6: usize = 72;
const SYS_dup: usize = 23;
const SYS_dup2: usize = 33;
const SYS_fcntl: usize = 25;
// socket 系统调用
const SYS_socket: usize = 198;
const SYS_bind: usize = 200;
const SYS_listen: usize = 201;
const SYS_accept: usize = 202;
const SYS_accept4: usize = 242;
const SYS_sendto: usize = 206;
const SYS_recvfrom: usize = 207;
const SYS_setsockopt: usize = 208;
const SYS_getsockopt: usize = 209;
const SYS_shutdown: usize = 210;
const SYS_sendmsg: usize = 211;
const SYS_recvmsg: usize = 212;
const SYS_connect: usize = 203;

const MMAP_BASE: usize = 0x5000_0000;

unsafe fn user_read(dst: *mut u8, src: usize, n: usize) {
    core::ptr::copy_nonoverlapping(src as *const u8, dst, n);
}
unsafe fn user_write(dst: usize, src: *const u8, n: usize) {
    core::ptr::copy_nonoverlapping(src, dst as *mut u8, n);
}

/// 从用户空间读 NUL 结尾字符串
unsafe fn read_user_string(ptr: usize, max: usize) -> Option<alloc::string::String> {
    use alloc::string::String;
    let mut buf = [0u8; 256];
    let mut len = 0;
    while len < max && len < buf.len() {
        let b = core::ptr::read_volatile((ptr + len) as *const u8);
        if b == 0 {
            break;
        }
        buf[len] = b;
        len += 1;
    }
    if len == 0 && max > 0 {
        // 看第一个字节
        let b = core::ptr::read_volatile(ptr as *const u8);
        if b != 0 {
            buf[0] = b;
            len = 1;
        }
    }
    Some(String::from(core::str::from_utf8(&buf[..len]).unwrap_or("")))
}

/// 取当前进程的 fd 表（syscall 路径）
fn with_fd_table<R>(f: impl FnOnce(&mut crate::vfs::FdTable) -> R) -> R {
    let p = current_process().expect("no current process in fd syscall");
    f(&mut p.fd_table)
}

#[no_mangle]
pub fn do_syscall(cx: &mut TrapContext) {
    let num = cx.x[17];
    crate::println!("[sys{}]", num);
    let a0 = cx.x[10];
    let a1 = cx.x[11];
    let a2 = cx.x[12];
    let a3 = cx.x[13];
    let ret: isize = match num {
        SYS_write => sys_write(a0, a1, a2),
        SYS_writev => sys_writev(a0, a1, a2),
        SYS_read => sys_read(a0, a1, a2),
        SYS_exit | SYS_exit_group => {
            crate::println!("[syscall] exit code={}", a0 as isize);
            crate::sched::exit_current(a0 as i32);
        }
        SYS_brk => sys_brk(a0),
        SYS_mmap => sys_mmap(a0, a1, a2, a3, cx.x[14], cx.x[15]),
        SYS_munmap => 0,
        SYS_mprotect => 0,
        SYS_getpid => crate::sched::current_pid() as isize,
        SYS_gettid => crate::sched::current_pid() as isize,
        SYS_getuid | SYS_geteuid | SYS_getgid | SYS_getegid => 1000,
        SYS_set_tid_address => {
            if let Some(p) = current_process() {
                p.tid_address = a0;
            }
            crate::sched::current_pid() as isize
        }
        SYS_set_robust_list => 0,
        SYS_futex => 0, // 占位：无真实 futex
        SYS_rt_sigaction | SYS_rt_sigprocmask => 0,
        SYS_ioctl => 0,
        SYS_uname => sys_uname(a0),
        SYS_clock_gettime => sys_clock_gettime(a0, a1),
        SYS_nanosleep => 0,
        SYS_getrandom => sys_getrandom(a0, a1, a2),
        SYS_prlimit64 => sys_prlimit64(a0, a1, a2, a3),
        SYS_close => {
            with_fd_table(|t| if t.close(a0) { 0isize } else { EBADF })
        }
        SYS_dup => EBADF,
        SYS_dup2 => EBADF,
        SYS_fcntl => 0,
        SYS_openat => sys_openat(a0, a1, a2, a3),
        SYS_fstat => sys_fstat(a0, a1),
        SYS_stat => sys_stat(a0, a1),
        SYS_newfstatat => sys_newfstatat(a0, a1, a2, a3),
        SYS_lseek => sys_lseek(a0, a1 as isize, a2),
        SYS_getcwd => {
            // 返回 "/"
            if a0 != 0 && a1 >= 2 {
                unsafe { core::ptr::write_volatile(a0 as *mut u8, b'/'); }
                unsafe { core::ptr::write_volatile((a0+1) as *mut u8, 0); }
                2
            } else {
                EINVAL
            }
        }
        SYS_readlink | SYS_readlinkat => ENOENT,
        SYS_clone => sys_clone(a0, a1, a2, cx.x[13]),
        SYS_execve => ENOSYS,
        SYS_clone3 => ENOSYS,
        SYS_ppoll | SYS_pselect6 | SYS_poll => sys_poll_stub(),
        SYS_socket => sys_socket(a0, a1, a2),
        SYS_bind => sys_bind(a0, a1, a2),
        SYS_listen => sys_listen(a0, a1),
        SYS_accept | SYS_accept4 => sys_accept(a0),
        SYS_connect => 0,
        SYS_sendto | SYS_sendmsg => sys_sendto(a0, a1, a2),
        SYS_recvfrom | SYS_recvmsg => sys_recvfrom(a0, a1, a2),
        SYS_setsockopt | SYS_getsockopt => 0,
        SYS_shutdown => 0,
        SYS_epoll_create1 => crate::net_stack::epoll_create(),
        SYS_epoll_ctl => crate::net_stack::epoll_ctl(a0, a1, a2, a3),
        SYS_epoll_pwait => crate::net_stack::epoll_wait(a0, a1, a2, a3),
        SYS_eventfd2 => sys_eventfd2(),
        SYS_pipe2 => sys_pipe2(a0),
        SYS_dup3 => sys_dup3(a0, a1),
        SYS_madvise => 0,
        SYS_getrlimit => sys_prlimit64(0, a0, 0, a1),
        SYS_setrlimit => 0,
        SYS_getrusage => 0,
        SYS_sched_yield => 0,
        SYS_sigaltstack => 0,
        SYS_rt_sigsuspend => {
            // worker 进程不阻塞（让它进入事件循环）；master 阻塞
            let pid = crate::sched::current_pid();
            if pid > 1 { 0 } else { -4 } // EINTR for worker, 阻塞 for master
        }
        SYS_rt_sigreturn => 0,
        SYS_setsid => 0,
        SYS_setgid | SYS_setuid | SYS_setpgid => 0,
        SYS_chdir => 0,
        SYS_chroot => 0,
        SYS_umask => 0,
        SYS_mkdirat => 0,
        SYS_pread64 => sys_pread64(a0, a1, a2, a3),
        SYS_pwrite64 => a2 as isize, // 假装写成功（丢弃）
        SYS_socketpair => sys_socketpair(a0, a1, a2, a3),
        SYS_sched_getaffinity => {
            // 写入空的 affinity mask（8 字节，CPU 0）
            if a2 != 0 {
                unsafe { core::ptr::write_volatile(a2 as *mut u8, 1); }
            }
            8
        }
        SYS_getppid => 1,
        SYS_sysinfo => 0,
        SYS_prctl => 0,
        SYS_unshare => 0,
        SYS_kill => 0,
        _ => {
            crate::println!("[syscall] unsupported num={}", num);
            ENOSYS
        }
    };
    cx.x[10] = ret as usize;
}

fn sys_write(fd: usize, buf: usize, count: usize) -> isize {
    // socket fd?
    let sock_id = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        p.sock_table.get(fd).map(|s| s.handle)
    };
    if let Some(id) = sock_id {
        let mut tmp = [0u8; 4096];
        let mut total = 0usize;
        let mut p = buf;
        let mut remaining = count;
        while remaining > 0 {
            let n = remaining.min(tmp.len());
            unsafe { user_read(tmp.as_mut_ptr(), p, n); }
            let sent = crate::net_stack::socket_send(id, &tmp[..n]);
            if sent == 0 { break; }
            p += sent;
            remaining -= sent;
            total += sent;
        }
        return total as isize;
    }
    // eventfd / pipe fd
    let kind = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        p.sock_table.get(fd).map(|s| s.kind.clone())
    };
    if let Some(k) = kind {
        if k == crate::process::SockKind::Eventfd || k == crate::process::SockKind::Pipe {
            return count as isize; // 假装写成功
        }
    }
    let mut tmp = [0u8; 512];
    let mut remaining = count;
    let mut p = buf;
    let mut total = 0usize;
    while remaining > 0 {
        let n = remaining.min(tmp.len());
        unsafe { user_read(tmp.as_mut_ptr(), p, n); }
        let wrote = with_fd_table(|t| t.write(fd, &tmp[..n]).unwrap_or(0));
        if wrote == 0 {
            break;
        }
        p += wrote;
        remaining -= wrote;
        total += wrote;
    }
    total as isize
}

fn sys_writev(fd: usize, iov: usize, iovcnt: usize) -> isize {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let base = unsafe { core::ptr::read_volatile((iov + i * 16) as *const usize) };
        let len = unsafe { core::ptr::read_volatile((iov + i * 16 + 8) as *const usize) };
        if base == 0 || len == 0 {
            continue;
        }
        let r = sys_write(fd, base, len);
        if r < 0 {
            return r;
        }
        total += r as usize;
    }
    total as isize
}

fn sys_read(fd: usize, buf: usize, count: usize) -> isize {
    // socket fd?
    let sock_id = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        p.sock_table.get(fd).map(|s| s.handle)
    };
    if let Some(id) = sock_id {
        let mut tmp = [0u8; 4096];
        let cap = tmp.len().min(count);
        let n = crate::net_stack::socket_recv(id, &mut tmp[..cap]);
        if n > 0 {
            unsafe { user_write(buf, tmp.as_ptr(), n); }
        }
        return n as isize;
    }
    // eventfd / pipe fd
    let kind = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        p.sock_table.get(fd).map(|s| s.kind.clone())
    };
    if let Some(k) = kind {
        if k == crate::process::SockKind::Eventfd {
            // read eventfd: 返回 8 字节 0
            let zeros = [0u8; 8];
            let n = 8.min(count);
            unsafe { user_write(buf, zeros.as_ptr(), n); }
            return n as isize;
        }
        if k == crate::process::SockKind::Pipe {
            return -11; // EAGAIN
        }
    }
    let mut tmp = [0u8; 512];
    let n = {
        let p = match current_process() { Some(p) => p, None => return ENOSYS };
        let sl = count.min(tmp.len());
        p.fd_table.read(fd, &mut tmp[..sl]).unwrap_or(0)
    };
    if n > 0 {
        unsafe { user_write(buf, tmp.as_ptr(), n); }
        let preview = core::str::from_utf8(&tmp[..n.min(40)]).unwrap_or("<binary>");
        crate::println!("[read fd={} n={} data={:?}]", fd, n, preview);
    }
    n as isize
}

fn sys_openat(_dirfd: usize, path: usize, flags: usize, _mode: usize) -> isize {
    let path_str = unsafe { read_user_string(path, 255) }.unwrap_or_default();
    crate::println!("[open {} flags={:#x}]", path_str, flags);
    let fd = with_fd_table(|t| t.open(&path_str, flags));
    match fd {
        Some(f) => f as isize,
        None => ENOENT,
    }
}

fn sys_lseek(fd: usize, offset: isize, whence: usize) -> isize {
    with_fd_table(|t| t.lseek(fd, offset, whence).map(|v| v as isize).unwrap_or(EBADF))
}

fn sys_fstat(fd: usize, statbuf: usize) -> isize {
    if with_fd_table(|t| t.stat(fd, statbuf)) { 0 } else { EBADF }
}

fn sys_stat(path: usize, statbuf: usize) -> isize {
    let p = unsafe { read_user_string(path, 255) }.unwrap_or_default();
    if crate::vfs::stat_path(&p, statbuf) { 0 } else { ENOENT }
}

fn sys_newfstatat(_dirfd: usize, path: usize, statbuf: usize, _flags: usize) -> isize {
    let p = unsafe { read_user_string(path, 255) }.unwrap_or_default();
    crate::println!("[stat {}]", p);
    if crate::vfs::stat_path(&p, statbuf) { 0 } else { ENOENT }
}

fn sys_brk(new: usize) -> isize {
    let p = match current_process() {
        Some(p) => p,
        None => return ENOSYS,
    };
    if new == 0 {
        return p.brk as isize;
    }
    if new < p.brk_start {
        return p.brk as isize;
    }
    // 扩展：映射 [old_top, new_top) 的页
    let old_top = (p.brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_top = (new + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let root = p.root_pa;
    let mut va = old_top;
    while va < new_top {
        let pa = crate::mm::frame::FRAME_ALLOCATOR.alloc_zeroed();
        if pa.is_none() {
            return p.brk as isize;
        }
        let pt = crate::mm::page_table::PageTable::from_root(root);
        pt.map_page(va, pa.unwrap(), crate::mm::page_table::PTE_R | crate::mm::page_table::PTE_W | crate::mm::page_table::PTE_U | crate::mm::page_table::PTE_A | crate::mm::page_table::PTE_D);
        va += PAGE_SIZE;
    }
    p.brk = new;
    p.brk as isize
}

fn sys_mmap(addr: usize, length: usize, _prot: usize, _flags: usize, _fd: usize, _off: usize) -> isize {
    if length == 0 {
        return EINVAL;
    }
    let p = match current_process() {
        Some(p) => p,
        None => return ENOSYS,
    };
    // 仅支持匿名映射；忽略 addr（由内核分配）
    let base = if addr != 0 && addr >= MMAP_BASE {
        addr
    } else {
        p.next_mmap
    };
    let pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;
    let root = p.root_pa;
    for i in 0..pages {
        let va = base + i * PAGE_SIZE;
        let pa = match crate::mm::frame::FRAME_ALLOCATOR.alloc_zeroed() {
            Some(pa) => pa,
            None => return ENOSYS,
        };
        let pt = crate::mm::page_table::PageTable::from_root(root);
        pt.map_page(va, pa, crate::mm::page_table::PTE_R | crate::mm::page_table::PTE_W | crate::mm::page_table::PTE_U | crate::mm::page_table::PTE_A | crate::mm::page_table::PTE_D);
    }
    // 推进 bump
    let next = base + pages * PAGE_SIZE;
    p.next_mmap = next;
    base as isize
}

fn sys_close(fd: usize) -> isize {
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    // 先查 socket 表
    let sock_id = p.sock_table.get(fd).map(|s| s.handle);
    if let Some(id) = sock_id {
        crate::net_stack::remove_socket(id);
        p.sock_table.close(fd);
        return 0;
    }
    // 再查文件表
    with_fd_table(|t| if t.close(fd) { 0isize } else { EBADF })
}

// === socket 系统调用 ===

fn sys_dup3(oldfd: usize, newfd: usize) -> isize {
    // 简化：不真正复制 fd，返回 newfd
    let _ = oldfd;
    newfd as isize
}

fn sys_poll_stub() -> isize {
    // 无就绪 fd
    0
}

fn sys_pread64(fd: usize, buf: usize, count: usize, offset: usize) -> isize {
    let mut tmp = [0u8; 4096];
    let cap = count.min(tmp.len());
    let n = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        p.fd_table.pread(fd, &mut tmp[..cap], offset).unwrap_or(0)
    };
    if n > 0 {
        unsafe { user_write(buf, tmp.as_ptr(), n); }
    }
    n as isize
}

fn sys_socketpair(_d: usize, _t: usize, _p: usize, sv: usize) -> isize {
    if sv != 0 {
        let p = match current_process() { Some(p) => p, None => return -3 };
        let fd1 = p.sock_table.alloc(crate::process::SockFd {
            handle: usize::MAX, local_port: 0, kind: crate::process::SockKind::Pipe,
        });
        let fd2 = p.sock_table.alloc(crate::process::SockFd {
            handle: usize::MAX, local_port: 0, kind: crate::process::SockKind::Pipe,
        });
        unsafe {
            core::ptr::write_volatile(sv as *mut i32, fd1 as i32);
            core::ptr::write_volatile((sv + 4) as *mut i32, fd2 as i32);
        }
        0
    } else {
        -14
    }
}

fn sys_eventfd2() -> isize {
    let p = match current_process() { Some(p) => p, None => return -3 };
    p.sock_table.alloc(crate::process::SockFd {
        handle: usize::MAX, local_port: 0, kind: crate::process::SockKind::Eventfd,
    }) as isize
}

fn sys_pipe2(fds: usize) -> isize {
    if fds == 0 { return -14; }
    let p = match current_process() { Some(p) => p, None => return -3 };
    let fd1 = p.sock_table.alloc(crate::process::SockFd {
        handle: usize::MAX, local_port: 0, kind: crate::process::SockKind::Pipe,
    });
    let fd2 = p.sock_table.alloc(crate::process::SockFd {
        handle: usize::MAX, local_port: 0, kind: crate::process::SockKind::Pipe,
    });
    unsafe {
        core::ptr::write_volatile(fds as *mut i32, fd1 as i32);
        core::ptr::write_volatile((fds + 4) as *mut i32, fd2 as i32);
    }
    0
}

fn sys_clone(flags: usize, stack: usize, ptid: usize, ctid: usize) -> isize {
    crate::sched::clone_child(flags, stack, ptid, ctid)
}

fn sys_socket(_domain: usize, _typ: usize, _proto: usize) -> isize {
    // 创建 smoltcp tcp socket
    let id = match crate::net_stack::new_tcp_socket() {
        Some(id) => id,
        None => return -12, // ENOMEM
    };
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    let fd = p.sock_table.alloc(crate::process::SockFd {
        handle: id,
        local_port: 0,
        kind: crate::process::SockKind::TcpStream,
    });
    fd as isize
}

/// 读 sockaddr_in: 2 字节 family + 2 字节 port(网络序) + 4 字节 addr
fn read_sockaddr_in(ptr: usize) -> Option<(u16, [u8; 4])> {
    if ptr == 0 {
        return None;
    }
    unsafe {
        let family = u16::from_le_bytes([
            core::ptr::read_volatile(ptr as *const u8),
            core::ptr::read_volatile((ptr + 1) as *const u8),
        ]);
        let port_be = [
            core::ptr::read_volatile((ptr + 2) as *const u8),
            core::ptr::read_volatile((ptr + 3) as *const u8),
        ];
        let port = u16::from_be_bytes(port_be);
        let mut addr = [0u8; 4];
        for i in 0..4 {
            addr[i] = core::ptr::read_volatile((ptr + 4 + i) as *const u8);
        }
        let _ = family;
        Some((port, addr))
    }
}

fn sys_bind(fd: usize, addr: usize, _len: usize) -> isize {
    let (port, _addr) = match read_sockaddr_in(addr) {
        Some(v) => v,
        None => return -14,
    };
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    let s = match p.sock_table.get(fd) {
        Some(s) => s,
        None => return -9,
    };
    s.local_port = port;
    crate::println!("[bind fd={} port={}]", fd, port);
    0
}

fn sys_listen(fd: usize, _backlog: usize) -> isize {
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    let (id, port) = {
        let s = match p.sock_table.get(fd) {
            Some(s) => s,
            None => return -9,
        };
        (s.handle, s.local_port)
    };
    if !crate::net_stack::listen_socket(id, port) {
        return -22;
    }
    if let Some(s) = p.sock_table.get(fd) {
        s.kind = crate::process::SockKind::TcpListener;
    }
    crate::println!("[listen fd={} id={} port={}]", fd, id, port);
    0
}

fn sys_accept(fd: usize) -> isize {
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    let (id, port) = {
        let s = match p.sock_table.get(fd) {
            Some(s) => s,
            None => return -9,
        };
        if s.kind != crate::process::SockKind::TcpListener {
            return -22;
        }
        (s.handle, s.local_port)
    };
    // 释放 current_process 借用
    drop(p);
    let new_id = match crate::net_stack::accept_socket(id, port) {
        Some(n) => n,
        None => return -11, // EAGAIN
    };
    let p = match current_process() {
        Some(p) => p,
        None => return -3,
    };
    let new_fd = p.sock_table.alloc(crate::process::SockFd {
        handle: new_id,
        local_port: port,
        kind: crate::process::SockKind::TcpStream,
    });
    new_fd as isize
}

fn sys_sendto(fd: usize, buf: usize, len: usize) -> isize {
    let id = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        match p.sock_table.get(fd) {
            Some(s) => s.handle,
            None => return -9,
        }
    };
    let mut tmp = [0u8; 4096];
    let mut total = 0usize;
    let mut p = buf;
    let mut remaining = len;
    while remaining > 0 {
        let n = remaining.min(tmp.len());
        unsafe { user_read(tmp.as_mut_ptr(), p, n); }
        let sent = crate::net_stack::socket_send(id, &tmp[..n]);
        if sent == 0 {
            break;
        }
        p += sent;
        remaining -= sent;
        total += sent;
    }
    total as isize
}

fn sys_recvfrom(fd: usize, buf: usize, len: usize) -> isize {
    let id = {
        let p = match current_process() { Some(p) => p, None => return -3 };
        match p.sock_table.get(fd) {
            Some(s) => s.handle,
            None => return -9,
        }
    };
    let mut tmp = [0u8; 4096];
    let cap = tmp.len().min(len);
    let n = crate::net_stack::socket_recv(id, &mut tmp[..cap]);
    if n > 0 {
        unsafe { user_write(buf, tmp.as_ptr(), n); }
    }
    n as isize
}

fn sys_uname(buf: usize) -> isize {
    if buf == 0 {
        return EFAULT;
    }
    let fields = [
        "Linux\0",
        "ijiege-os\0",
        "5.15.0\0",
        "#1 SMP ijiege\0",
        "riscv64\0",
        "\0",
    ];
    let mut off = buf;
    for f in fields.iter() {
        let bytes = f.as_bytes();
        for &b in bytes {
            unsafe { core::ptr::write_volatile(off as *mut u8, b); }
            off += 1;
        }
        // 填充到 65 字节
        let pad = 65 - bytes.len();
        for _ in 0..pad {
            unsafe { core::ptr::write_volatile(off as *mut u8, 0); }
            off += 1;
        }
    }
    0
}

fn sys_clock_gettime(_clk: usize, tp: usize) -> isize {
    if tp == 0 {
        return EFAULT;
    }
    let cycles = crate::timer::read_cycles();
    let sec = cycles / 10_000_000;
    let nsec = (cycles % 10_000_000) * 100;
    unsafe {
        core::ptr::write_volatile(tp as *mut usize, sec);
        core::ptr::write_volatile((tp + 8) as *mut usize, nsec);
    }
    0
}

fn sys_getrandom(buf: usize, len: usize, _flags: usize) -> isize {
    if buf == 0 {
        return EFAULT;
    }
    let cycles = crate::timer::read_cycles();
    let mut state = cycles.wrapping_mul(6364136223846793005).wrapping_add(1);
    for i in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        unsafe { core::ptr::write_volatile((buf + i) as *mut u8, state as u8); }
    }
    len as isize
}

fn sys_prlimit64(_pid: usize, resource: usize, newlim: usize, oldlim: usize) -> isize {
    if oldlim == 0 {
        return EFAULT;
    }
    // rlimit: rlim_cur, rlim_max (各 8 字节)
    let (cur, max) = match resource {
        3 => (0x7fffffffffffffffusize, 0x7fffffffffffffffusize), // RLIMIT_STACK
        7 => (-1isize as usize, -1isize as usize),                // RLIMIT_NOFILE
        _ => (0x7fffffffffffffffusize, 0x7fffffffffffffffusize),
    };
    unsafe {
        core::ptr::write_volatile(oldlim as *mut usize, cur);
        core::ptr::write_volatile((oldlim + 8) as *mut usize, max);
    }
    let _ = newlim;
    0
}
