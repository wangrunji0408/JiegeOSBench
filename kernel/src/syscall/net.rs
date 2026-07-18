use crate::fs::{self, pipe, File};
use crate::mm::translated_byte_buffer;
use crate::net::{self, socket::TcpFile};
use crate::task::{current_task, current_user_token};
use alloc::sync::Arc;
use smoltcp::wire::{IpAddress, IpEndpoint};

const AF_UNIX: u16 = 1;
const AF_INET: u16 = 2;
const SOCK_TYPE_MASK: usize = 0xff;
const SOCK_STREAM: usize = 1;

fn read_u16_be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn read_u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_sockaddr(token: usize, ptr: *const u8, len: usize) -> Option<(u16, IpEndpoint)> {
    if len < 8 || ptr.is_null() {
        return None;
    }
    let mut raw = [0u8; 16];
    let n = len.min(16);
    let chunks = translated_byte_buffer(token, ptr, n);
    let mut off = 0;
    for c in chunks {
        raw[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    let family = u16::from_ne_bytes([raw[0], raw[1]]);
    let port = read_u16_be(&raw[2..4]);
    let addr = read_u32_be(&raw[4..8]);
    Some((
        family,
        IpEndpoint::new(
            IpAddress::v4((addr >> 24) as u8, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8),
            port,
        ),
    ))
}

fn write_sockaddr(token: usize, ptr: *mut u8, len_ptr: *mut u8, ep: IpEndpoint) {
    if ptr.is_null() {
        return;
    }
    let mut raw = [0u8; 16];
    raw[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    raw[2..4].copy_from_slice(&ep.port.to_be_bytes());
    if let IpAddress::Ipv4(v4) = ep.addr {
        raw[4..8].copy_from_slice(&v4.0);
    }
    let mut chunks = translated_byte_buffer(token, ptr, 16);
    let mut copied = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&raw[copied..copied + n]);
        copied += n;
    }
    if !len_ptr.is_null() {
        let mut lc = translated_byte_buffer(token, len_ptr, 4);
        let val = 16u32.to_ne_bytes();
        let mut copied = 0;
        for c in lc.iter_mut() {
            let n = c.len();
            c.copy_from_slice(&val[copied..copied + n]);
            copied += n;
        }
    }
}

pub fn sys_socket(domain: usize, ty: usize, _protocol: usize) -> isize {
    let stype = ty & SOCK_TYPE_MASK;
    if domain == AF_INET as usize && stype == SOCK_STREAM {
        if !net::is_available() {
            return -97; // EAFNOSUPPORT-ish: no network device
        }
        let file: Arc<dyn File> = Arc::new(TcpFile::new());
        let task = current_task().unwrap();
        return task.inner_lock().alloc_fd(file) as isize;
    }
    if domain == AF_UNIX as usize {
        // No real AF_UNIX support; nginx only uses this to probe for an
        // (absent) external channel path, which is expected to fail at
        // `connect()` -- handing back a fd that just always errors there
        // reproduces that gracefully-handled failure.
        let file: Arc<dyn File> = Arc::new(TcpFile::new());
        let task = current_task().unwrap();
        return task.inner_lock().alloc_fd(file) as isize;
    }
    -97
}

pub fn sys_bind(fd: usize, addr: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() else {
        return -88; // ENOTSOCK
    };
    match read_sockaddr(token, addr, len) {
        Some((_, ep)) => {
            tcp.bind(ep.port);
            0
        }
        None => -22,
    }
}

pub fn sys_listen(fd: usize, _backlog: usize) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    match file.as_any().downcast_ref::<TcpFile>() {
        Some(tcp) => {
            tcp.listen().ok();
            0
        }
        None => -88,
    }
}

pub fn sys_accept4(fd: usize, addr: *mut u8, len_ptr: *mut u8, _flags: usize) -> isize {
    crate::println!("[dbg] accept4(fd={}) called by pid {}", fd, crate::task::current_pid());
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() else {
        return -88;
    };
    if let Err(e) = fs::wait_readable(&file) {
        return e;
    }
    match tcp.accept() {
        Some(handle) => {
            if let Some(ep) = net::socket::accepted_endpoint(handle) {
                let token = current_user_token();
                write_sockaddr(token, addr, len_ptr, ep);
            }
            let newfile: Arc<dyn File> = Arc::new(TcpFile::from_accepted(handle));
            task.inner_lock().alloc_fd(newfile) as isize
        }
        None => -11, // EAGAIN (shouldn't usually happen after wait_readable, but be safe)
    }
}

pub fn sys_connect(fd: usize, addr: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() else {
        return -88;
    };
    match read_sockaddr(token, addr, len) {
        Some((family, ep)) if family == AF_INET => {
            if tcp.connect(ep).is_ok() {
                0
            } else {
                -111 // ECONNREFUSED
            }
        }
        _ => -2, // treat AF_UNIX / bad addr connect as ENOENT, matching the probe nginx expects to fail
    }
}

pub fn sys_getsockname(fd: usize, addr: *mut u8, len_ptr: *mut u8) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() {
        if let Some(ep) = tcp.local_endpoint() {
            write_sockaddr(current_user_token(), addr, len_ptr, ep);
        }
    }
    0
}

pub fn sys_getpeername(fd: usize, addr: *mut u8, len_ptr: *mut u8) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() {
        if let Some(ep) = tcp.remote_endpoint() {
            write_sockaddr(current_user_token(), addr, len_ptr, ep);
        }
    }
    0
}

pub fn sys_setsockopt() -> isize {
    0
}

pub fn sys_getsockopt(_fd: usize, _level: usize, _optname: usize, optval: *mut u8, optlen_ptr: *mut u8) -> isize {
    if !optval.is_null() {
        let token = current_user_token();
        let mut c = translated_byte_buffer(token, optval, 4);
        for chunk in c.iter_mut() {
            chunk.fill(0);
        }
    }
    let _ = optlen_ptr;
    0
}

pub fn sys_shutdown(fd: usize, _how: usize) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() {
        tcp.shutdown();
    }
    0
}

pub fn sys_recvfrom(fd: usize, buf: *mut u8, len: usize, _flags: usize, addr: *mut u8, len_ptr: *mut u8) -> isize {
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Err(e) = fs::wait_readable(&file) {
        return e;
    }
    if !addr.is_null() {
        if let Some(tcp) = file.as_any().downcast_ref::<TcpFile>() {
            if let Some(ep) = tcp.remote_endpoint() {
                write_sockaddr(current_user_token(), addr, len_ptr, ep);
            }
        }
    }
    let token = current_user_token();
    let mut chunks = translated_byte_buffer(token, buf, len);
    let mut total = 0;
    for c in chunks.iter_mut() {
        let n = file.read(c);
        total += n;
        if n < c.len() {
            break;
        }
    }
    total as isize
}

pub fn sys_sendto(fd: usize, buf: *const u8, len: usize, _flags: usize) -> isize {
    let token = current_user_token();
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Err(e) = fs::wait_writable(&file) {
        return e;
    }
    let chunks = translated_byte_buffer(token, buf, len);
    let mut total = 0;
    for c in chunks {
        total += file.write(c);
    }
    total as isize
}

pub fn sys_socketpair(domain: usize, _ty: usize, _protocol: usize, fds_ptr: *mut u8) -> isize {
    if domain != AF_UNIX as usize {
        return -97;
    }
    let (a, b) = pipe::pair();
    let task = current_task().unwrap();
    let (fd_a, fd_b) = {
        let mut inner = task.inner_lock();
        let fd_a = inner.alloc_fd(Arc::new(a));
        let fd_b = inner.alloc_fd(Arc::new(b));
        (fd_a, fd_b)
    };
    let token = current_user_token();
    let bytes = [(fd_a as u32).to_ne_bytes(), (fd_b as u32).to_ne_bytes()].concat();
    let mut chunks = translated_byte_buffer(token, fds_ptr, 8);
    let mut copied = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
    }
    0
}

struct MsgHdr {
    iov_base: usize,
    iov_len: usize,
}

fn read_msghdr(token: usize, msg: *const u8) -> Option<MsgHdr> {
    let chunks = translated_byte_buffer(token, msg, 32);
    let mut raw = [0u8; 32];
    let mut off = 0;
    for c in chunks {
        raw[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    let iov = usize::from_ne_bytes(raw[16..24].try_into().unwrap());
    let iovlen = usize::from_ne_bytes(raw[24..32].try_into().unwrap());
    if iov == 0 || iovlen == 0 {
        return None;
    }
    let iov_entry = translated_byte_buffer(token, iov as *const u8, 16);
    let mut raw2 = [0u8; 16];
    let mut off2 = 0;
    for c in iov_entry {
        raw2[off2..off2 + c.len()].copy_from_slice(c);
        off2 += c.len();
    }
    Some(MsgHdr {
        iov_base: usize::from_ne_bytes(raw2[0..8].try_into().unwrap()),
        iov_len: usize::from_ne_bytes(raw2[8..16].try_into().unwrap()),
    })
}

pub fn sys_sendmsg(fd: usize, msg: *const u8, _flags: usize) -> isize {
    let token = current_user_token();
    let Some(hdr) = read_msghdr(token, msg) else {
        return 0;
    };
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let chunks = translated_byte_buffer(token, hdr.iov_base as *const u8, hdr.iov_len);
    let mut total = 0;
    for c in chunks {
        total += file.write(c);
    }
    total as isize
}

pub fn sys_recvmsg(fd: usize, msg: *const u8, _flags: usize) -> isize {
    let token = current_user_token();
    let Some(hdr) = read_msghdr(token, msg) else {
        return 0;
    };
    let task = current_task().unwrap();
    let file = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    if let Err(e) = fs::wait_readable(&file) {
        return e;
    }
    let mut chunks = translated_byte_buffer(token, hdr.iov_base as *const u8, hdr.iov_len);
    let mut total = 0;
    for c in chunks.iter_mut() {
        let n = file.read(c);
        total += n;
        if n < c.len() {
            break;
        }
    }
    total as isize
}
