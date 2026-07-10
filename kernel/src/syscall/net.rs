/// 网络相关syscall

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use lazy_static::lazy_static;

use smoltcp::{
    socket::{tcp, udp},
    wire::{IpAddress, IpEndpoint, Ipv4Address},
};

use crate::task::{current_task, manager::TASK_MANAGER};
use crate::task::process::{FileDesc, TaskState};
use crate::net::socket::KernelSocket;

use super::*;

// AF families
const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;
const AF_INET6: i32 = 10;

// Socket types
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;

// sockaddr_in布局
#[repr(C)]
struct SockaddrIn {
    sin_family: u16,
    sin_port: u16, // big-endian
    sin_addr: u32, // big-endian
    sin_zero: [u8; 8],
}

fn parse_sockaddr(task_memory: &crate::mm::MemorySet, addr_va: usize, addrlen: u32) -> Option<IpEndpoint> {
    if addr_va == 0 || addrlen < 8 {
        return None;
    }

    let mut buf = [0u8; 16];
    task_memory.copy_from_user(addr_va, &mut buf[..addrlen.min(16) as usize]);

    let family = u16::from_le_bytes(buf[0..2].try_into().unwrap());

    match family as i32 {
        AF_INET => {
            let port = u16::from_be_bytes(buf[2..4].try_into().unwrap());
            let addr = [buf[4], buf[5], buf[6], buf[7]];
            // Use Unspecified for 0.0.0.0 to allow binding/listening on any interface
            let ip_addr = if addr == [0, 0, 0, 0] {
                IpAddress::v4(0, 0, 0, 0)
            } else {
                IpAddress::Ipv4(Ipv4Address::new(addr[0], addr[1], addr[2], addr[3]))
            };
            Some(IpEndpoint::new(ip_addr, port))
        }
        _ => None,
    }
}

fn write_sockaddr(task_memory: &crate::mm::MemorySet, addr_va: usize, addrlen_va: usize, endpoint: IpEndpoint) {
    if addr_va == 0 { return; }

    let mut buf = [0u8; 16];
    buf[0] = AF_INET as u8;
    buf[1] = 0;
    let port = endpoint.port.to_be_bytes();
    buf[2] = port[0];
    buf[3] = port[1];

    if let IpAddress::Ipv4(addr) = endpoint.addr {
        let octets = addr.octets();
        buf[4..8].copy_from_slice(&octets);
    }

    task_memory.copy_to_user(addr_va, &buf[..16]);

    if addrlen_va != 0 {
        task_memory.copy_to_user(addrlen_va, &16u32.to_le_bytes());
    }
}

pub fn sys_socket(domain: i32, sock_type: i32, protocol: i32) -> isize {
    let ty = sock_type & !SOCK_NONBLOCK & !SOCK_CLOEXEC;

    let fd = match (domain, ty) {
        (AF_INET, SOCK_STREAM) => {
            crate::net::socket::create_tcp_socket()
        }
        (AF_INET, SOCK_DGRAM) => {
            crate::net::socket::create_udp_socket()
        }
        (AF_UNIX, _) => {
            // Unix domain socket - 用pipe模拟
            return sys_socket_unix(ty);
        }
        _ => {
            println!("[net] Unsupported socket domain={} type={}", domain, ty);
            return EAFNOSUPPORT;
        }
    };

    if fd < 0 { return ENOMEM; }

    // 注册到进程的fd表
    let task = current_task().unwrap();
    let mut t = task.lock();
    let proc_fd = t.alloc_fd();
    t.fds.insert(proc_fd, FileDesc::Socket(fd));

    let is_nonblock = sock_type & SOCK_NONBLOCK != 0;
    // TODO: 设置非阻塞标志

    proc_fd as isize
}

fn sys_socket_unix(ty: i32) -> isize {
    // 简化：创建一个内部的"socket"用于进程间通信
    // 这里返回一个特殊的fd
    let buf = Arc::new(Mutex::new(Vec::new()));
    let task = current_task().unwrap();
    let mut t = task.lock();
    let fd = t.alloc_fd();
    // 暂用pipe模拟
    t.fds.insert(fd, FileDesc::Pipe { read_end: true, buf });
    fd as isize
}

const EAFNOSUPPORT: isize = -97;

pub fn sys_bind(fd: i32, addr_va: usize, addrlen: u32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        _ => return ENOTSOCK,
    };

    let endpoint = match parse_sockaddr(&t.memory_set, addr_va, addrlen) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    drop(t);

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let mut sock = sock.lock();
    match &mut *sock {
        KernelSocket::Tcp { local_addr, .. } => {
            *local_addr = Some(endpoint);

            // 在smoltcp中绑定
            let mut net = crate::net::NETWORK.lock();
            if let Some(state) = net.as_mut() {
                // 找到socket handle
                if let KernelSocket::Tcp { handle, .. } = &*sock {
                    // 已经在分配时创建了
                }
            }
            0
        }
        KernelSocket::Udp { local_addr, handle } => {
            *local_addr = Some(endpoint);
            let mut net = crate::net::NETWORK.lock();
            if let Some(state) = net.as_mut() {
                let socket = state.sockets.get_mut::<udp::Socket>(*handle);
                socket.bind(endpoint).map(|_| 0isize).unwrap_or(EADDRINUSE)
            } else {
                ENETDOWN
            }
        }
    }
}

const ENOTSOCK: isize = -88;
const ENETDOWN: isize = -100;

pub fn sys_listen(fd: i32, backlog: i32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        _ => return ENOTSOCK,
    };

    drop(t);

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let (handle, ep) = {
        let mut sock = sock.lock();
        match &mut *sock {
            KernelSocket::Tcp { handle, local_addr, is_listener, .. } => {
                *is_listener = true;
                (*handle, *local_addr)
            }
            _ => return EOPNOTSUPP,
        }
    };

    // 在smoltcp中将socket设置为监听状态
    if let Some(ep) = ep {
        // Use IpListenEndpoint with None addr to accept on any interface (like 0.0.0.0)
        let listen_ep = smoltcp::wire::IpListenEndpoint {
            addr: if ep.addr == IpAddress::Ipv4(Ipv4Address::new(0, 0, 0, 0)) {
                None
            } else {
                Some(ep.addr)
            },
            port: ep.port,
        };
        let mut net = crate::net::NETWORK.lock();
        if let Some(state) = net.as_mut() {
            let socket = state.sockets.get_mut::<tcp::Socket>(handle);
            socket.listen(listen_ep).map(|_| 0isize).unwrap_or(EADDRINUSE)
        } else {
            ENETDOWN
        }
    } else {
        EINVAL
    }
}

const EOPNOTSUPP: isize = -95;

pub fn sys_accept(fd: i32, addr_va: usize, addrlen_va: usize) -> isize {
    sys_accept4(fd, addr_va, addrlen_va, 0)
}

pub fn sys_accept4(fd: i32, addr_va: usize, addrlen_va: usize, flags: i32) -> isize {
    let task = current_task().unwrap();
    let socket_fd = {
        let t = task.lock();
        match t.fds.get(&fd) {
            Some(FileDesc::Socket(s)) => *s,
            _ => return ENOTSOCK,
        }
    };

    let handle = {
        let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
            Some(s) => s,
            None => return EBADF,
        };
        let sock = sock.lock();
        match &*sock {
            KernelSocket::Tcp { handle, .. } => *handle,
            _ => return EOPNOTSUPP,
        }
    };

    // 等待新连接
    loop {
        crate::net::poll();

        let remote = {
            let mut net = crate::net::NETWORK.lock();
            if let Some(state) = net.as_mut() {
                let socket = state.sockets.get::<tcp::Socket>(handle);
                if socket.is_active() && !socket.is_listening() {
                    socket.remote_endpoint()
                } else {
                    None
                }
            } else {
                return ENETDOWN;
            }
        };

        if let Some(remote) = remote {
            // The ORIGINAL smoltcp socket is now CONNECTED (transitioned from LISTEN)
            // We need to:
            // 1. Use the original socket handle for the accepted connection
            // 2. Create a NEW smoltcp socket in LISTEN state for future connections

            // Get the local address for re-listening
            let local_addr = {
                let sock = crate::net::socket::get_socket_by_fd(socket_fd).unwrap();
                let sock = sock.lock();
                match &*sock {
                    KernelSocket::Tcp { local_addr, .. } => *local_addr,
                    _ => None,
                }
            };

            // Create a new LISTEN socket to replace the original
            let new_listen_handle = {
                let rx_buf = tcp::SocketBuffer::new(alloc::vec![0u8; 65536]);
                let tx_buf = tcp::SocketBuffer::new(alloc::vec![0u8; 65536]);
                let mut new_listen_sock = tcp::Socket::new(rx_buf, tx_buf);
                // Put the new socket in LISTEN state immediately
                if let Some(la) = local_addr {
                    // Use IpListenEndpoint with None addr to accept on any interface
                    let listen_ep = smoltcp::wire::IpListenEndpoint {
                        addr: if la.addr == IpAddress::Ipv4(Ipv4Address::new(0, 0, 0, 0)) {
                            None
                        } else {
                            Some(la.addr)
                        },
                        port: la.port,
                    };
                    let _ = new_listen_sock.listen(listen_ep);
                }
                let mut net = crate::net::NETWORK.lock();
                let state = net.as_mut().unwrap();
                state.sockets.add(new_listen_sock)
            };

            // Update the original kernel socket to use the new listen handle
            // and create a new "connected" kernel socket using the original handle
            {
                let sock = crate::net::socket::get_socket_by_fd(socket_fd).unwrap();
                let mut sock = sock.lock();
                if let KernelSocket::Tcp { handle: orig_handle, local_addr: la, is_listener, .. } = &mut *sock {
                    // The "new" accepted fd uses the ORIGINAL smoltcp handle (which is now CONNECTED)
                    let accepted_fd = crate::net::socket::alloc_socket_fd(KernelSocket::Tcp {
                        handle: *orig_handle,
                        local_addr: *la,
                        remote_addr: Some(remote),
                        is_listener: false,
                    });

                    // Update the listener socket to use the NEW smoltcp handle
                    *orig_handle = new_listen_handle;
                    *is_listener = true;

                    // Register to process fd table
                    if addr_va != 0 {
                        let t = task.lock();
                        write_sockaddr(&t.memory_set, addr_va, addrlen_va, remote);
                    }
                    let mut t = task.lock();
                    let proc_fd = t.alloc_fd();
                    t.fds.insert(proc_fd, FileDesc::Socket(accepted_fd));
                    return proc_fd as isize;
                }
            }

            return EINVAL;
        }

        // 没有新连接，让出CPU
        let pid = task.lock().pid;
        {
            let mut mgr = TASK_MANAGER.lock();
            if let Some(t) = mgr.tasks.get(&pid) {
                t.lock().state = TaskState::Blocking;
            }
        }
        crate::task::schedule();
    }
}

pub fn sys_connect(fd: i32, addr_va: usize, addrlen: u32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        Some(FileDesc::Pipe { .. }) => {
            // AF_UNIX socket (implemented as pipe)
            // Note: can't access task memory here because t (task lock) is already held
            // Read sun_path before the match
            drop(t); // Release lock to read memory
            let t3 = task.lock();
            let mut sun_path = [0u8; 108];
            let read_len = ((addrlen as usize).saturating_sub(2)).min(108);
            if read_len > 0 {
                t3.memory_set.copy_from_user(addr_va + 2, &mut sun_path[..read_len]);
            }
            drop(t3);

            // Handle abstract namespace (first byte is '\0')
            let is_abstract = sun_path[0] == 0;
            let path_str = if is_abstract {
                let end = sun_path[1..read_len.max(1)].iter().position(|&b| b == 0).unwrap_or(read_len.saturating_sub(1));
                core::str::from_utf8(&sun_path[1..1+end]).unwrap_or("?abstract?")
            } else {
                let end = sun_path[..read_len].iter().position(|&b| b == 0).unwrap_or(read_len);
                core::str::from_utf8(&sun_path[..end]).unwrap_or("?")
            };
            println!("[connect] AF_UNIX path={}", path_str);

            if path_str.contains("nscd") || is_abstract {
                return ECONNREFUSED;
            }
            if path_str.contains("log") || path_str.contains("syslog") {
                return 0;
            }
            return ECONNREFUSED;
        }
        _ => return ENOTSOCK,
    };

    let remote = match parse_sockaddr(&t.memory_set, addr_va, addrlen) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    drop(t);

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let handle = {
        let sock = sock.lock();
        match &*sock {
            KernelSocket::Tcp { handle, .. } => *handle,
            _ => return EOPNOTSUPP,
        }
    };

    {
        let mut net = crate::net::NETWORK.lock();
        if let Some(state) = net.as_mut() {
            let socket = state.sockets.get_mut::<tcp::Socket>(handle);
            socket.connect(state.iface.context(), remote, 49152u16).ok();
        }
    }

    0
}

pub fn sys_getsockname(fd: i32, addr_va: usize, addrlen_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        _ => return ENOTSOCK,
    };

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let sock = sock.lock();
    match &*sock {
        KernelSocket::Tcp { local_addr, .. } => {
            if let Some(ep) = local_addr {
                write_sockaddr(&t.memory_set, addr_va, addrlen_va, *ep);
            } else {
                write_sockaddr(&t.memory_set, addr_va, addrlen_va,
                    IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::UNSPECIFIED), 0));
            }
        }
        _ => {}
    }
    0
}

pub fn sys_getpeername(fd: i32, addr_va: usize, addrlen_va: usize) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        _ => return ENOTSOCK,
    };

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let sock = sock.lock();
    match &*sock {
        KernelSocket::Tcp { remote_addr, .. } => {
            if let Some(ep) = remote_addr {
                write_sockaddr(&t.memory_set, addr_va, addrlen_va, *ep);
                0
            } else {
                ENOTCONN
            }
        }
        _ => ENOTCONN,
    }
}

pub fn sys_setsockopt(fd: i32, level: i32, optname: i32, optval_va: usize, optlen: u32) -> isize {
    // 忽略大多数socket选项，只处理关键的
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_REUSEPORT: i32 = 15;
    const SO_KEEPALIVE: i32 = 9;
    const TCP_NODELAY: i32 = 1;
    const IPPROTO_TCP: i32 = 6;

    0 // 全部忽略，返回成功
}

pub fn sys_getsockopt(fd: i32, level: i32, optname: i32, optval_va: usize, optlen_va: usize) -> isize {
    const SOL_SOCKET: i32 = 1;
    const SO_ERROR: i32 = 4;
    const SO_TYPE: i32 = 3;

    let task = current_task().unwrap();
    let t = task.lock();

    match (level, optname) {
        (SOL_SOCKET, SO_ERROR) => {
            t.memory_set.copy_to_user(optval_va, &0i32.to_le_bytes());
            t.memory_set.copy_to_user(optlen_va, &4u32.to_le_bytes());
        }
        (SOL_SOCKET, SO_TYPE) => {
            t.memory_set.copy_to_user(optval_va, &(SOCK_STREAM as i32).to_le_bytes());
            t.memory_set.copy_to_user(optlen_va, &4u32.to_le_bytes());
        }
        _ => {
            t.memory_set.copy_to_user(optval_va, &0i32.to_le_bytes());
            t.memory_set.copy_to_user(optlen_va, &4u32.to_le_bytes());
        }
    }
    0
}

pub fn sys_sendto(fd: i32, buf_va: usize, len: usize, flags: i32, addr_va: usize, addrlen: u32) -> isize {
    let task = current_task().unwrap();
    let socket_fd = {
        let t = task.lock();
        match t.fds.get(&fd) {
            Some(FileDesc::Socket(s)) => *s,
            Some(FileDesc::File { .. }) | Some(FileDesc::Stdout) | Some(FileDesc::Stderr) => {
                return crate::syscall::fs::sys_write(fd, buf_va, len);
            }
            Some(FileDesc::Pipe { .. }) => {
                // AF_UNIX socket (pipe) - discard data, return success
                // Print syslog message for debugging
                if len > 0 && len < 512 {
                    let t = task.lock();
                    let mut buf = alloc::vec![0u8; len];
                    t.memory_set.copy_from_user(buf_va, &mut buf);
                    if let Ok(s) = core::str::from_utf8(&buf) {
                        println!("[syslog] {}", s);
                    }
                }
                return len as isize;
            }
            _ => return EBADF,
        }
    };

    let buf = {
        let t = task.lock();
        let mut buf = vec![0u8; len];
        t.memory_set.copy_from_user(buf_va, &mut buf);
        buf
    };

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let handle = {
        let sock = sock.lock();
        match &*sock {
            KernelSocket::Tcp { handle, .. } => *handle,
            KernelSocket::Udp { handle, .. } => {
                // UDP发送
                let remote = {
                    let t = task.lock();
                    parse_sockaddr(&t.memory_set, addr_va, addrlen)
                };
                let mut net = crate::net::NETWORK.lock();
                if let Some(state) = net.as_mut() {
                    let socket = state.sockets.get_mut::<udp::Socket>(*handle);
                    if let Some(ep) = remote {
                        socket.send_slice(&buf, ep).ok();
                    }
                }
                return len as isize;
            }
        }
    };

    // TCP发送
    loop {
        crate::net::poll();

        let sent = {
            let mut net = crate::net::NETWORK.lock();
            if let Some(state) = net.as_mut() {
                let socket = state.sockets.get_mut::<tcp::Socket>(handle);
                if socket.can_send() {
                    let n = socket.send_slice(&buf).unwrap_or(0);
                    Some(n)
                } else {
                    None
                }
            } else {
                return ENETDOWN;
            }
        };

        if let Some(n) = sent {
            return n as isize;
        }

        // 等待可写
        let pid = task.lock().pid;
        {
            let mut mgr = TASK_MANAGER.lock();
            if let Some(t) = mgr.tasks.get(&pid) {
                t.lock().state = TaskState::Blocking;
            }
        }
        crate::task::schedule();
    }
}

pub fn sys_recvfrom(fd: i32, buf_va: usize, len: usize, flags: i32, addr_va: usize, addrlen_va: usize) -> isize {
    const MSG_DONTWAIT: i32 = 0x40;

    let task = current_task().unwrap();
    let socket_fd = {
        let t = task.lock();
        match t.fds.get(&fd) {
            Some(FileDesc::Socket(s)) => *s,
            _ => return crate::syscall::fs::sys_read(fd, buf_va, len),
        }
    };

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let handle = {
        let sock = sock.lock();
        match &*sock {
            KernelSocket::Tcp { handle, .. } => *handle,
            _ => return ENOTSUP,
        }
    };

    loop {
        crate::net::poll();

        let (data, remote) = {
            let mut net = crate::net::NETWORK.lock();
            if let Some(state) = net.as_mut() {
                let socket = state.sockets.get_mut::<tcp::Socket>(handle);
                if socket.can_recv() {
                    let remote = socket.remote_endpoint();
                    let mut buf = vec![0u8; len];
                    let n = socket.recv_slice(&mut buf).unwrap_or(0);
                    (Some((buf, n)), remote)
                } else if !socket.is_open() {
                    return 0; // EOF
                } else {
                    (None, None)
                }
            } else {
                return ENETDOWN;
            }
        };

        if let Some((buf, n)) = data {
            let t = task.lock();
            t.memory_set.copy_to_user(buf_va, &buf[..n]);
            if let Some(remote) = remote {
                write_sockaddr(&t.memory_set, addr_va, addrlen_va, remote);
            }
            return n as isize;
        }

        if flags & MSG_DONTWAIT != 0 {
            return EAGAIN;
        }

        // 等待数据
        let pid = task.lock().pid;
        {
            let mut mgr = TASK_MANAGER.lock();
            if let Some(t) = mgr.tasks.get(&pid) {
                t.lock().state = TaskState::Blocking;
            }
        }
        crate::task::schedule();
    }
}

const ENOTSUP: isize = -95;  // EOPNOTSUPP

pub fn sys_sendmsg(fd: i32, msg_va: usize, flags: i32) -> isize {
    // 简化：读取msghdr，只处理第一个iov
    let task = current_task().unwrap();
    let t = task.lock();

    // struct msghdr {
    //   msg_name: *void,     // 0
    //   msg_namelen: u32,    // 8
    //   _pad: u32,
    //   msg_iov: *iovec,     // 16
    //   msg_iovlen: usize,   // 24
    //   ...
    // }
    let mut msghdr = [0u8; 56];
    t.memory_set.copy_from_user(msg_va, &mut msghdr);

    let msg_iov = usize::from_le_bytes(msghdr[16..24].try_into().unwrap());
    let msg_iovlen = usize::from_le_bytes(msghdr[24..32].try_into().unwrap());

    let mut total = 0isize;
    for i in 0..msg_iovlen {
        let mut iov = [0u8; 16];
        t.memory_set.copy_from_user(msg_iov + i * 16, &mut iov);
        let base = usize::from_le_bytes(iov[0..8].try_into().unwrap());
        let len = usize::from_le_bytes(iov[8..16].try_into().unwrap());
        drop(t);
        let n = sys_sendto(fd, base, len, flags, 0, 0);
        if n < 0 { return n; }
        total += n;
        let t2 = task.lock();
        // 重新借用
        break;
    }
    total
}

pub fn sys_recvmsg(fd: i32, msg_va: usize, flags: i32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let mut msghdr = [0u8; 56];
    t.memory_set.copy_from_user(msg_va, &mut msghdr);

    let msg_iov = usize::from_le_bytes(msghdr[16..24].try_into().unwrap());
    let msg_iovlen = usize::from_le_bytes(msghdr[24..32].try_into().unwrap());

    if msg_iovlen == 0 { return 0; }

    let mut iov = [0u8; 16];
    t.memory_set.copy_from_user(msg_iov, &mut iov);
    let base = usize::from_le_bytes(iov[0..8].try_into().unwrap());
    let len = usize::from_le_bytes(iov[8..16].try_into().unwrap());
    drop(t);

    sys_recvfrom(fd, base, len, flags, 0, 0)
}

pub fn sys_shutdown(fd: i32, how: i32) -> isize {
    let task = current_task().unwrap();
    let t = task.lock();

    let socket_fd = match t.fds.get(&fd) {
        Some(FileDesc::Socket(s)) => *s,
        _ => return ENOTSOCK,
    };

    drop(t);

    let sock = match crate::net::socket::get_socket_by_fd(socket_fd) {
        Some(s) => s,
        None => return EBADF,
    };

    let handle = {
        let sock = sock.lock();
        match &*sock {
            KernelSocket::Tcp { handle, .. } => *handle,
            _ => return 0,
        }
    };

    let mut net = crate::net::NETWORK.lock();
    if let Some(state) = net.as_mut() {
        let socket = state.sockets.get_mut::<tcp::Socket>(handle);
        socket.close();
    }

    0
}

pub fn sys_socketpair(domain: i32, ty: i32, protocol: i32, sv_va: usize) -> isize {
    // 用两个pipe模拟
    let buf1 = Arc::new(Mutex::new(Vec::new()));
    let buf2 = Arc::new(Mutex::new(Vec::new()));

    let task = current_task().unwrap();
    let mut t = task.lock();

    let fd1 = t.alloc_fd();
    t.fds.insert(fd1, FileDesc::Pipe { read_end: true, buf: buf1.clone() });
    let fd2 = t.alloc_fd();
    t.fds.insert(fd2, FileDesc::Pipe { read_end: false, buf: buf2 });

    let fds = [fd1 as u32, fd2 as u32];
    t.memory_set.copy_to_user(sv_va, bytemuck_cast(&fds));
    0
}

// epoll实现
struct Epoll {
    fds: BTreeMap<i32, u32>, // fd -> events
}

lazy_static! {
    static ref EPOLLS: Mutex<BTreeMap<i32, Epoll>> = Mutex::new(BTreeMap::new());
    static ref NEXT_EPOLL_FD: Mutex<i32> = Mutex::new(100);
}

pub fn sys_epoll_create1(flags: i32) -> isize {
    let mut next = NEXT_EPOLL_FD.lock();
    let fd = *next;
    *next += 1;
    EPOLLS.lock().insert(fd, Epoll { fds: BTreeMap::new() });

    // 注册到进程fd表（用特殊的socket fd）
    let task = current_task().unwrap();
    let mut t = task.lock();
    let proc_fd = t.alloc_fd();
    // 暂时用pipe模拟
    t.fds.insert(proc_fd, FileDesc::Pipe {
        read_end: true,
        buf: Arc::new(Mutex::new(Vec::new())),
    });

    proc_fd as isize
}

pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event_va: usize) -> isize {
    const EPOLL_CTL_ADD: i32 = 1;
    const EPOLL_CTL_DEL: i32 = 2;
    const EPOLL_CTL_MOD: i32 = 3;

    // Store the epoll event registration in the task's epoll table
    // struct epoll_event { u32 events; u64 data; } = 12 bytes
    let task = match current_task() {
        Some(t) => t,
        None => return EINVAL,
    };
    let mut t = task.lock();

    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if event_va != 0 {
                let mut event_buf = [0u8; 12];
                t.memory_set.copy_from_user(event_va, &mut event_buf);
                let events = u32::from_le_bytes(event_buf[0..4].try_into().unwrap());
                let data = u64::from_le_bytes(event_buf[4..12].try_into().unwrap());
                // Store in epoll_table: fd -> (events, data)
                t.epoll_table.insert(fd, (events, data));
            }
            0
        }
        EPOLL_CTL_DEL => {
            t.epoll_table.remove(&fd);
            0
        }
        _ => EINVAL,
    }
}

pub fn sys_epoll_pwait(epfd: i32, events_va: usize, maxevents: i32, timeout: i32, sigmask: usize) -> isize {
    if timeout == 0 {
        return 0;
    }

    // 简化：等待timeout毫秒，轮询所有socket
    let end = if timeout > 0 {
        crate::timer::get_time_ms() + timeout as usize
    } else {
        usize::MAX
    };

    let task = current_task().unwrap();
    let mut events_count = 0;
    let mut events_buf = vec![0u8; maxevents as usize * 12]; // sizeof(epoll_event) = 12

    loop {
        crate::net::poll();

        // 检查所有socket是否有事件
        let fds: Vec<(i32, i32)> = {
            let t = task.lock();
            t.fds.iter().filter_map(|(&fd, desc)| {
                match desc {
                    FileDesc::Socket(_) => Some((fd, 0)),
                    _ => None,
                }
            }).collect()
        };

        for (fd, _) in &fds {
            // 检查是否可读
            let readable = check_fd_readable(*fd);
            if readable {
                if events_count < maxevents {
                    // 写入epoll_event - use data stored via epoll_ctl
                    let offset = events_count as usize * 12;
                    let event_data = {
                        let t = task.lock();
                        t.epoll_table.get(fd).map(|&(events, data)| (events, data))
                    };
                    let (ev_flags, ev_data) = event_data.unwrap_or((1u32, *fd as u64));
                    events_buf[offset..offset+4].copy_from_slice(&ev_flags.to_le_bytes());
                    events_buf[offset+4..offset+12].copy_from_slice(&ev_data.to_le_bytes());
                    events_count += 1;
                }
            }
        }

        if events_count > 0 {
            let t = task.lock();
            t.memory_set.copy_to_user(events_va, &events_buf[..events_count as usize * 12]);
            return events_count as isize;
        }

        if crate::timer::get_time_ms() >= end {
            return 0;
        }

        // 等待
        let pid = task.lock().pid;
        {
            let until = (crate::timer::get_time_ms() + 10).min(end);
            let mut mgr = TASK_MANAGER.lock();
            if let Some(t) = mgr.tasks.get(&pid) {
                t.lock().state = TaskState::Sleeping(until);
            }
        }
        crate::task::schedule();
    }
}

fn check_fd_readable(fd: i32) -> bool {
    let task = match current_task() {
        Some(t) => t,
        None => return false,
    };
    let t = task.lock();

    match t.fds.get(&fd) {
        Some(FileDesc::Socket(socket_fd)) => {
            let sock = match crate::net::socket::get_socket_by_fd(*socket_fd) {
                Some(s) => s,
                None => return false,
            };
            let sock = sock.lock();
            match &*sock {
                KernelSocket::Tcp { handle, is_listener, .. } => {
                    if *is_listener {
                        // 检查是否有新连接
                        let net = crate::net::NETWORK.lock();
                        if let Some(state) = net.as_ref() {
                            let socket = state.sockets.get::<tcp::Socket>(*handle);
                            socket.is_active() && !socket.is_listening()
                        } else {
                            false
                        }
                    } else {
                        let net = crate::net::NETWORK.lock();
                        if let Some(state) = net.as_ref() {
                            let socket = state.sockets.get::<tcp::Socket>(*handle);
                            socket.can_recv()
                        } else {
                            false
                        }
                    }
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub fn sys_poll(fds_va: usize, nfds: u32, timeout: i32) -> isize {
    // struct pollfd { fd: i32, events: i16, revents: i16 }
    let task = current_task().unwrap();

    let end = if timeout >= 0 {
        crate::timer::get_time_ms() + timeout as usize
    } else {
        usize::MAX
    };

    loop {
        crate::net::poll();

        let mut count = 0i32;
        for i in 0..nfds as usize {
            let mut pollfd = [0u8; 8];
            {
                let t = task.lock();
                t.memory_set.copy_from_user(fds_va + i * 8, &mut pollfd);
            }
            let fd = i32::from_le_bytes(pollfd[0..4].try_into().unwrap());
            let events = i16::from_le_bytes(pollfd[4..6].try_into().unwrap());

            const POLLIN: i16 = 0x0001;
            const POLLOUT: i16 = 0x0004;
            const POLLERR: i16 = 0x0008;
            const POLLHUP: i16 = 0x0010;

            let mut revents: i16 = 0;

            if fd < 0 { continue; }

            if events & POLLIN != 0 && check_fd_readable(fd) {
                revents |= POLLIN;
                count += 1;
            }
            if events & POLLOUT != 0 {
                // 大多数时候可写
                revents |= POLLOUT;
                count += 1;
            }

            let t = task.lock();
            t.memory_set.copy_to_user(fds_va + i * 8 + 6, &revents.to_le_bytes());
        }

        if count > 0 || crate::timer::get_time_ms() >= end {
            return count as isize;
        }

        // 短暂等待
        let pid = task.lock().pid;
        {
            let until = (crate::timer::get_time_ms() + 10).min(end);
            let mut mgr = TASK_MANAGER.lock();
            if let Some(t) = mgr.tasks.get(&pid) {
                t.lock().state = TaskState::Sleeping(until);
            }
        }
        crate::task::schedule();
    }
}

pub fn sys_pselect6(nfds: i32, readfds_va: usize, writefds_va: usize, exceptfds_va: usize, timeout_va: usize, sigmask_va: usize) -> isize {
    // 简化实现
    if timeout_va != 0 {
        let task = current_task().unwrap();
        let t = task.lock();
        let mut ts = [0u8; 16];
        t.memory_set.copy_from_user(timeout_va, &mut ts);
        let sec = i64::from_le_bytes(ts[0..8].try_into().unwrap());
        let nsec = i64::from_le_bytes(ts[8..16].try_into().unwrap());
        let ms = sec as usize * 1000 + nsec as usize / 1_000_000;
        if ms > 0 {
            drop(t);
            let pid = task.lock().pid;
            let until = crate::timer::get_time_ms() + ms;
            {
                let mut mgr = TASK_MANAGER.lock();
                if let Some(t) = mgr.tasks.get(&pid) {
                    t.lock().state = TaskState::Sleeping(until);
                }
            }
            crate::task::schedule();
        }
    }
    0
}

pub fn sys_inotify_init1(flags: i32) -> isize {
    // 返回一个假的fd
    let task = current_task().unwrap();
    let mut t = task.lock();
    let fd = t.alloc_fd();
    t.fds.insert(fd, FileDesc::Pipe {
        read_end: true,
        buf: Arc::new(Mutex::new(Vec::new())),
    });
    fd as isize
}

pub fn sys_timerfd_create(clockid: i32, flags: i32) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();
    let fd = t.alloc_fd();
    t.fds.insert(fd, FileDesc::Pipe {
        read_end: true,
        buf: Arc::new(Mutex::new(Vec::new())),
    });
    fd as isize
}

pub fn sys_eventfd2(initval: u32, flags: i32) -> isize {
    let task = current_task().unwrap();
    let mut t = task.lock();
    let fd = t.alloc_fd();
    let mut buf = Vec::new();
    buf.extend_from_slice(&(initval as u64).to_le_bytes());
    t.fds.insert(fd, FileDesc::Pipe {
        read_end: true,
        buf: Arc::new(Mutex::new(buf)),
    });
    fd as isize
}

pub fn sys_recvmmsg(fd: i32, msgvec_va: usize, vlen: u32, flags: i32, timeout_va: usize) -> isize {
    // 简化：只接收一个消息
    0
}

pub fn sys_sendmmsg(fd: i32, msgvec_va: usize, vlen: u32, flags: i32) -> isize {
    0
}

fn bytemuck_cast<T>(s: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(
            s.as_ptr() as *const u8,
            s.len() * core::mem::size_of::<T>(),
        )
    }
}
