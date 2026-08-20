//! fd 抽象层：console / 文件 / TCP socket / epoll
//! 以及 socket 系列与 epoll 系列系统调用的内核实现

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;

use crate::errno::{Errno, Ret};
use crate::vfs::FileData;
use super::stack;

// epoll 事件位
pub const EPOLLIN: u32 = 0x001;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLL_CTL_ADD: usize = 1;
pub const EPOLL_CTL_DEL: usize = 2;
pub const EPOLL_CTL_MOD: usize = 3;

#[derive(Clone)]
pub enum SockState {
    Idle,
    Listening { port: u16 },
    Connected { handle: SocketHandle },
}

pub struct TcpSock {
    pub state: SockState,
    pub bind_port: Option<u16>,
    pub nonblocking: bool,
    pub nodelay: bool,
    /// 记录 EOF 已上报
    pub rclosed: bool,
}

impl TcpSock {
    pub fn new() -> Self {
        TcpSock {
            state: SockState::Idle,
            bind_port: None,
            nonblocking: false,
            nodelay: false,
            rclosed: false,
        }
    }
}

pub struct EpollState {
    pub items: BTreeMap<usize, (u32, u64)>, // fd -> (interest, data)
}

pub enum FdEntry {
    Console,
    Null,
    File {
        data: FileData,
        pos: usize,
        append: bool,
    },
    Socket(Rc<RefCell<TcpSock>>),
    Epoll(Rc<RefCell<EpollState>>),
}

impl FdEntry {
    pub fn console() -> FdEntry {
        FdEntry::Console
    }
}

// ---------------- socket syscalls ----------------

pub fn sys_socket(domain: usize, ty: usize, _proto: usize) -> Ret {
    if domain != 2 && domain != 10 {
        // AF_INET / AF_INET6
        return Err(Errno::Eafnosupport);
    }
    if domain == 10 {
        return Err(Errno::Eafnosupport);
    }
    let ty = ty & 0xff; // 去 SOCK_NONBLOCK/CLOEXEC
    if ty != 1 {
        return Err(Errno::Eprotonosupport);
    }
    let fd = crate::proc::alloc_fd();
    let proc = crate::proc::current();
    proc.fds[fd] = Some(FdEntry::Socket(Rc::new(RefCell::new(TcpSock::new()))));
    Ok(fd)
}

pub fn sys_bind(fd: usize, addr: &[u8]) -> Ret {
    // sockaddr_in: family u16, port u16 BE, addr u32
    if addr.len() < 8 {
        return Err(Errno::Einval);
    }
    let port = u16::from_be_bytes([addr[2], addr[3]]);
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            let mut st = s.borrow_mut();
            if !matches!(st.state, SockState::Idle) {
                return Err(Errno::Einval);
            }
            st.bind_port = Some(port);
            Ok(0)
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_listen(fd: usize, backlog: usize) -> Ret {
    let _ = backlog;
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            // 已在监听：幂等返回（nginx 会二次 listen 调整 backlog）
            if matches!(s.borrow().state, SockState::Listening { .. }) {
                return Ok(0);
            }
            let port = s.borrow().bind_port.ok_or(Errno::Edestaddrreq)?;
            if !stack::listen(port) {
                return Err(Errno::Eaddrinuse);
            }
            s.borrow_mut().state = SockState::Listening { port };
            Ok(0)
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_accept4(fd: usize, addr_out: Option<&mut [u8]>, nonblock: bool) -> Ret {
    let port = {
        let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
        match entry {
            FdEntry::Socket(s) => {
                let st = s.borrow();
                match &st.state {
                    SockState::Listening { port } => *port,
                    _ => return Err(Errno::Einval),
                }
            }
            _ => return Err(Errno::Enotsock),
        }
    };
    if let Some(h) = stack::take_established(port) {
        let newfd = crate::proc::alloc_fd();
        let proc = crate::proc::current();
        let mut sock = TcpSock::new();
        sock.state = SockState::Connected { handle: h };
        sock.nonblocking = nonblock;
        proc.fds[newfd] = Some(FdEntry::Socket(Rc::new(RefCell::new(sock))));
        stack::register_connection(h, newfd);
        // 输出对端地址（slirp: 10.0.2.2:xxxx）
        if let Some(a) = addr_out {
            if a.len() >= 8 {
                a[0..2].copy_from_slice(&2u16.to_le_bytes());
                a[2..4].copy_from_slice(&12345u16.to_be_bytes());
                a[4..8].copy_from_slice(&[10, 0, 2, 2]);
            }
        }
        Ok(newfd)
    } else {
        Err(Errno::Eagain)
    }
}

pub fn sys_connect(fd: usize, _addr: &[u8]) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(_) => Err(Errno::Enetunreach), // 不支持客户端连接（nginx 场景不需要）
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_setsockopt(fd: usize, level: usize, optname: usize, optval: &[u8]) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            // SOL_SOCKET=1: SO_REUSEADDR=2, SO_SNDBUF/RCVBUF, SO_KEEPALIVE
            // IPPROTO_TCP=6: TCP_NODELAY=1
            if level == 6 && optname == 1 {
                s.borrow_mut().nodelay = !optval.is_empty() && optval[0] != 0;
            }
            Ok(0)
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_getsockopt(fd: usize, level: usize, optname: usize, optval: &mut [u8]) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            if optval.is_empty() {
                return Err(Errno::Einval);
            }
            if level == 1 && optname == 3 {
                // SO_TYPE = SOCK_STREAM
                optval[0..4].copy_from_slice(&1u32.to_le_bytes());
            } else if level == 1 && optname == 4 {
                // SO_ERROR
                optval[0..4].copy_from_slice(&0u32.to_le_bytes());
            } else if level == 6 && optname == 1 {
                let on = if s.borrow().nodelay { 1u32 } else { 0 };
                optval[0..4].copy_from_slice(&on.to_le_bytes());
            } else {
                optval[0..4].copy_from_slice(&0u32.to_le_bytes());
            }
            Ok(0)
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_recv(fd: usize, buf: &mut [u8], _flags: usize) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            let handle = match &s.borrow().state {
                SockState::Connected { handle } => *handle,
                _ => return Err(Errno::Enotconn),
            };
            let n = stack::net();
            let sock = n.sockets.get_mut::<tcp::Socket>(handle);
            if sock.can_recv() {
                let len = sock.recv_slice(buf);
                if len == Ok(0) && buf.len() > 0 {
                    // EOF
                    return Ok(0);
                }
                match len {
                    Ok(k) => Ok(k),
                    Err(_) => Err(Errno::Eio),
                }
            } else {
                let state = sock.state();
                if state == tcp::State::Closed
                    || state == tcp::State::CloseWait
                    || state == tcp::State::FinWait2
                {
                    Ok(0) // EOF
                } else {
                    Err(Errno::Eagain)
                }
            }
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_send(fd: usize, buf: &[u8], _flags: usize) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            let handle = match &s.borrow().state {
                SockState::Connected { handle } => *handle,
                _ => return Err(Errno::Enotconn),
            };
            let n = stack::net();
            let sock = n.sockets.get_mut::<tcp::Socket>(handle);
            let state = sock.state();
            if state == tcp::State::Closed || state == tcp::State::CloseWait {
                return Err(Errno::Epipe);
            }
            if sock.can_send() {
                match sock.send_slice(buf) {
                    Ok(k) => {
                        // 立即尝试发包
                        super::stack::net_poll();
                        Ok(k)
                    }
                    Err(_) => Err(Errno::Eio),
                }
            } else {
                Err(Errno::Eagain)
            }
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_shutdown(fd: usize, how: usize) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            let handle = match &s.borrow().state {
                SockState::Connected { handle } => *handle,
                _ => return Ok(0),
            };
            let n = stack::net();
            let sock = n.sockets.get_mut::<tcp::Socket>(handle);
            if how == 1 || how == 2 {
                sock.close(); // SHUT_WR: 发 FIN
            } else {
                sock.close();
            }
            super::stack::net_poll();
            Ok(0)
        }
        _ => Err(Errno::Enotsock),
    }
}

pub fn sys_close_fd(fd: usize) -> Ret {
    let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            match &s.borrow().state {
                SockState::Listening { port } => {
                    stack::remove_listener(*port);
                }
                SockState::Connected { handle } => {
                    stack::close_connection(*handle);
                    super::stack::net_poll();
                }
                SockState::Idle => {}
            }
            Ok(0)
        }
        _ => Ok(0), // 其他类型无需清理
    }
}

// ---------------- epoll ----------------

pub fn sys_epoll_create1() -> Ret {
    let fd = crate::proc::alloc_fd();
    let proc = crate::proc::current();
    proc.fds[fd] = Some(FdEntry::Epoll(Rc::new(RefCell::new(EpollState {
        items: BTreeMap::new(),
    }))));
    Ok(fd)
}

pub fn sys_epoll_ctl(epfd: usize, op: usize, target_fd: usize, events: u32, data: u64) -> Ret {
    let entry = crate::proc::get_fd(epfd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Epoll(ep) => {
            // 目标 fd 必须存在
            crate::proc::get_fd(target_fd).ok_or(Errno::Ebadf)?;
            let mut ep = ep.borrow_mut();
            match op {
                EPOLL_CTL_ADD => {
                    if ep.items.contains_key(&target_fd) {
                        return Err(Errno::Eexist);
                    }
                    ep.items.insert(target_fd, (events, data));
                    Ok(0)
                }
                EPOLL_CTL_MOD => {
                    if !ep.items.contains_key(&target_fd) {
                        return Err(Errno::Enoent);
                    }
                    ep.items.insert(target_fd, (events, data));
                    Ok(0)
                }
                EPOLL_CTL_DEL => {
                    if ep.items.remove(&target_fd).is_none() {
                        return Err(Errno::Enoent);
                    }
                    Ok(0)
                }
                _ => Err(Errno::Einval),
            }
        }
        _ => Err(Errno::Einval),
    }
}

/// 计算 fd 的就绪事件位（0 = 不就绪）
pub fn fd_ready_events(fd: usize) -> u32 {
    let entry = match crate::proc::get_fd(fd) {
        Some(e) => e,
        None => return EPOLLERR,
    };
    match entry {
        FdEntry::Socket(s) => {
            let st = s.borrow();
            match &st.state {
                SockState::Listening { port } => {
                    if stack::has_established(*port) {
                        EPOLLIN
                    } else {
                        0
                    }
                }
                SockState::Connected { handle } => {
                    let n = stack::net();
                    let sock = n.sockets.get::<tcp::Socket>(*handle);
                    let mut ev = 0u32;
                    if sock.can_recv() {
                        ev |= EPOLLIN;
                    }
                    let state = sock.state();
                    match state {
                        tcp::State::Closed
                        | tcp::State::CloseWait
                        | tcp::State::FinWait2
                        | tcp::State::LastAck
                        | tcp::State::Closing
                        | tcp::State::TimeWait => {
                            ev |= EPOLLIN | EPOLLRDHUP;
                        }
                        _ => {}
                    }
                    if sock.can_send() {
                        ev |= EPOLLOUT;
                    }
                    ev
                }
                SockState::Idle => 0,
            }
        }
        FdEntry::Console => EPOLLOUT | EPOLLIN,
        FdEntry::File { .. } => EPOLLOUT, // 文件随时可写; 可读需 pos < len
        FdEntry::Null => 0,
        FdEntry::Epoll(_) => 0,
    }
}

/// epoll_wait：收集就绪事件（会先 poll 网络栈），阻塞语义由调用方循环实现
pub fn sys_epoll_collect(epfd: usize, events_out: &mut [(u32, u64)]) -> usize {
    super::stack::net_poll();
    let entry = match crate::proc::get_fd(epfd) {
        Some(e) => e,
        None => return 0,
    };
    match entry {
        FdEntry::Epoll(ep) => {
            let ep = ep.borrow();
            let mut count = 0usize;
            for (&fd, &(interest, data)) in ep.items.iter() {
                if count >= events_out.len() {
                    break;
                }
                let ready = fd_ready_events(fd);
                let matched = ready & (interest | EPOLLERR | EPOLLHUP);
                if matched != 0 {
                    events_out[count] = (matched, data);
                    count += 1;
                }
            }
            count
        }
        _ => 0,
    }
}

/// epoll_wait 完整实现（含阻塞等待）
pub fn sys_epoll_wait(epfd: usize, events_out: &mut [(u32, u64)], timeout_ms: i64) -> Ret {
    let deadline = if timeout_ms < 0 {
        None
    } else {
        Some(crate::trap::now_ms() as i64 + timeout_ms)
    };
    loop {
        let n = sys_epoll_collect(epfd, events_out);
        if n > 0 {
            return Ok(n);
        }
        let now = crate::trap::now_ms() as i64;
        match deadline {
            Some(d) if now >= d => return Ok(0),
            Some(d) => {
                let wait = core::cmp::min((d - now) as u64, 10);
                super::stack::wait_ms(wait);
            }
            None => {
                // 无限等：10ms 心跳轮询（网络无中断，只能轮询 virtio）
                super::stack::wait_ms(10);
            }
        }
    }
}

/// sendfile: 文件 -> socket
pub fn sys_sendfile(out_fd: usize, in_fd: usize, count: usize) -> Ret {
    // 源文件数据
    let file_data = {
        let entry = crate::proc::get_fd(in_fd).ok_or(Errno::Ebadf)?;
        match entry {
            FdEntry::File { data, pos, .. } => match data {
                FileData::Static(b) => {
                    let start = *pos;
                    let end = core::cmp::min(start + count, b.len());
                    let slice = &b[start..end];
                    slice.to_vec()
                }
                FileData::Tmp(v) => {
                    let v = v.borrow();
                    let start = *pos;
                    let end = core::cmp::min(start + count, v.len());
                    v[start..end].to_vec()
                }
            },
            _ => return Err(Errno::Einval),
        }
    };
    if file_data.is_empty() {
        return Ok(0);
    }
    // 目标 socket
    let entry = crate::proc::get_fd(out_fd).ok_or(Errno::Ebadf)?;
    match entry {
        FdEntry::Socket(s) => {
            let handle = match &s.borrow().state {
                SockState::Connected { handle } => *handle,
                _ => return Err(Errno::Enotconn),
            };
            let n = stack::net();
            let sock = n.sockets.get_mut::<tcp::Socket>(handle);
            let state = sock.state();
            if state == tcp::State::Closed || state == tcp::State::CloseWait {
                return Err(Errno::Epipe);
            }
            if !sock.can_send() {
                super::stack::net_poll();
                let sock = stack::net().sockets.get_mut::<tcp::Socket>(handle);
                if !sock.can_send() {
                    return Err(Errno::Eagain);
                }
            }
            let sock = stack::net().sockets.get_mut::<tcp::Socket>(handle);
            let sent = sock
                .send_slice(&file_data)
                .map_err(|_| Errno::Eio)?;
            // 更新文件 pos
            let entry = crate::proc::get_fd(in_fd).unwrap();
            if let FdEntry::File { pos, .. } = entry {
                *pos += sent;
            }
            super::stack::net_poll();
            Ok(sent)
        }
        _ => Err(Errno::Einval),
    }
}

/// ioctl 处理：FIONBIO
pub fn sys_ioctl(fd: usize, cmd: usize, arg: usize) -> Ret {
    const FIONBIO: usize = 0x5421;
    match cmd {
        FIONBIO => {
            let entry = crate::proc::get_fd(fd).ok_or(Errno::Ebadf)?;
            match entry {
                FdEntry::Socket(s) => {
                    let on = unsafe { *(arg as *const u32) } != 0;
                    s.borrow_mut().nonblocking = on;
                    Ok(0)
                }
                _ => Ok(0),
            }
        }
        _ => Err(Errno::Enotty),
    }
}
