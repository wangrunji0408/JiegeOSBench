//! 文件描述符层：普通文件 / 控制台 / socket / epoll / 管道

use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const O_NONBLOCK: u32 = 0o4000;
pub const O_CLOEXEC: u32 = 0o2000000;
pub const O_APPEND: u32 = 0o2000;

const FDFLAGS_NONBLOCK: usize = 1;
const FDFLAGS_CLOEXEC: usize = 2;

pub struct Fd {
    pub kind: FdKind,
    pub flags: AtomicUsize,
}

impl Fd {
    pub fn new(kind: FdKind) -> Self {
        Self {
            kind,
            flags: AtomicUsize::new(0),
        }
    }
    pub fn nonblock(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & FDFLAGS_NONBLOCK != 0
    }
    pub fn set_nonblock(&self, v: bool) {
        let mut f = self.flags.load(Ordering::Relaxed);
        if v {
            f |= FDFLAGS_NONBLOCK;
        } else {
            f &= !FDFLAGS_NONBLOCK;
        }
        self.flags.store(f, Ordering::Relaxed);
    }
    pub fn cloexec(&self) -> bool {
        self.flags.load(Ordering::Relaxed) & FDFLAGS_CLOEXEC != 0
    }
    pub fn set_cloexec(&self, v: bool) {
        let mut f = self.flags.load(Ordering::Relaxed);
        if v {
            f |= FDFLAGS_CLOEXEC;
        } else {
            f &= !FDFLAGS_CLOEXEC;
        }
        self.flags.store(f, Ordering::Relaxed);
    }

    /// 轮询就绪状态：(readable, writable, error)
    pub fn poll(&self) -> (bool, bool, bool) {
        match &self.kind {
            FdKind::Stdin => (crate::sbi::console_getchar().is_some(), false, false),
            FdKind::Stdout | FdKind::Stderr => (false, true, false),
            FdKind::File(_) => (true, true, false),
            FdKind::Socket(id) => crate::net::poll_socket(*id),
            FdKind::PipeRead(pipe) => {
                let p = pipe.inner.lock();
                (!p.buf.is_empty() || p.write_closed, false, false)
            }
            FdKind::PipeWrite(pipe) => {
                let p = pipe.inner.lock();
                (false, p.buf.len() < p.cap || p.read_closed, p.read_closed)
            }
            FdKind::Eventfd(ef) => {
                let c = ef.count.lock();
                (*c > 0, *c < u64::MAX - 1, false)
            }
            FdKind::UnixStream(pair, is_a) => {
                let (inbox, outbox, peer_closed) = if *is_a {
                    (&pair.b_to_a, &pair.a_to_b, &pair.b_closed)
                } else {
                    (&pair.a_to_b, &pair.b_to_a, &pair.a_closed)
                };
                let readable = !inbox.lock().is_empty() || *peer_closed.lock();
                let writable = outbox.lock().len() < 65536 || *peer_closed.lock();
                (readable, writable, false)
            }
            FdKind::Epoll(_) => (false, false, false),
        }
    }
}

pub enum FdKind {
    Stdin,
    Stdout,
    Stderr,
    File(FileFd),
    Socket(usize),
    Epoll(usize),
    PipeRead(Arc<Pipe>),
    PipeWrite(Arc<Pipe>),
    Eventfd(Arc<Eventfd>),
    UnixStream(Arc<UnixStream>, bool), // (pair, is_side_a)
}

/// eventfd
pub struct Eventfd {
    pub count: UPIntrFreeCell<u64>,
}

impl Eventfd {
    pub fn new(init: u64) -> Self {
        Self {
            count: unsafe { UPIntrFreeCell::new(init) },
        }
    }
}

/// AF_UNIX stream socketpair 的一对小端点
pub struct UnixStream {
    pub a_to_b: UPIntrFreeCell<alloc::collections::VecDeque<u8>>,
    pub b_to_a: UPIntrFreeCell<alloc::collections::VecDeque<u8>>,
    pub a_closed: UPIntrFreeCell<bool>,
    pub b_closed: UPIntrFreeCell<bool>,
}

impl UnixStream {
    pub fn new() -> Self {
        Self {
            a_to_b: unsafe { UPIntrFreeCell::new(alloc::collections::VecDeque::new()) },
            b_to_a: unsafe { UPIntrFreeCell::new(alloc::collections::VecDeque::new()) },
            a_closed: unsafe { UPIntrFreeCell::new(false) },
            b_closed: unsafe { UPIntrFreeCell::new(false) },
        }
    }
}

pub struct FileFd {
    pub node: usize,
    pub offset: UPIntrFreeCell<usize>,
    pub readable: bool,
    pub writable: bool,
    pub append: bool,
    pub path: String,
}

impl FileFd {
    pub fn new(node: usize, readable: bool, writable: bool, append: bool, path: String) -> Self {
        Self {
            node,
            offset: unsafe { UPIntrFreeCell::new(0) },
            readable,
            writable,
            append,
            path,
        }
    }
}

/// 管道
pub struct Pipe {
    pub inner: UPIntrFreeCell<PipeInner>,
}

pub struct PipeInner {
    pub buf: alloc::collections::VecDeque<u8>,
    pub cap: usize,
    pub read_closed: bool,
    pub write_closed: bool,
}

impl Pipe {
    pub fn new() -> Self {
        Self {
            inner: unsafe {
                UPIntrFreeCell::new(PipeInner {
                    buf: alloc::collections::VecDeque::new(),
                    cap: 65536,
                    read_closed: false,
                    write_closed: false,
                })
            },
        }
    }
}

/// epoll 实例
pub struct EpollInstance {
    pub interests: BTreeMap<usize, EpollEvent>,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

pub const EPOLLIN: u32 = 0x1;
pub const EPOLLOUT: u32 = 0x4;
pub const EPOLLERR: u32 = 0x8;
pub const EPOLLHUP: u32 = 0x10;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLET: u32 = 1 << 31;

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

lazy_static::lazy_static! {
    static ref EPOLL_TABLE: UPIntrFreeCell<Vec<Option<EpollInstance>>> =
        unsafe { UPIntrFreeCell::new(Vec::new()) };
}

pub fn epoll_create() -> usize {
    let mut table = EPOLL_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(EpollInstance {
                interests: BTreeMap::new(),
            });
            return i;
        }
    }
    table.push(Some(EpollInstance {
        interests: BTreeMap::new(),
    }));
    table.len() - 1
}

pub fn epoll_ctl(id: usize, op: i32, fd: usize, event: Option<EpollEvent>) -> i32 {
    let mut table = EPOLL_TABLE.lock();
    let inst = match table.get_mut(id).and_then(|s| s.as_mut()) {
        Some(i) => i,
        None => return -9, // EBADF
    };
    match op {
        EPOLL_CTL_ADD => {
            if inst.interests.contains_key(&fd) {
                return -17; // EEXIST
            }
            inst.interests.insert(fd, event.unwrap());
            0
        }
        EPOLL_CTL_MOD => {
            if !inst.interests.contains_key(&fd) {
                return -2; // ENOENT
            }
            inst.interests.insert(fd, event.unwrap());
            0
        }
        EPOLL_CTL_DEL => {
            inst.interests.remove(&fd);
            0
        }
        _ => -22, // EINVAL
    }
}

pub fn epoll_close(id: usize) {
    EPOLL_TABLE.lock()[id] = None;
}

/// 从所有 epoll 实例中移除某个 fd（Linux：close 自动移除）
pub fn epoll_remove_fd(fd: usize) {
    let mut table = EPOLL_TABLE.lock();
    for slot in table.iter_mut() {
        if let Some(inst) = slot.as_mut() {
            inst.interests.remove(&fd);
        }
    }
}

/// 收集就绪事件（调用方持有 fd_table 来查询）
pub fn epoll_collect(
    id: usize,
    fd_table: &Vec<Option<Arc<Fd>>>,
    out: &mut Vec<EpollEvent>,
    max: usize,
) -> usize {
    let table = EPOLL_TABLE.lock();
    let inst = match table.get(id).and_then(|s| s.as_ref()) {
        Some(i) => i,
        None => return 0,
    };
    let mut count = 0;
    for (&fd, ev) in inst.interests.iter() {
        if count >= max {
            break;
        }
        if let Some(Some(f)) = fd_table.get(fd) {
            let (r, w, e) = f.poll();
            let mut revents = 0u32;
            if r && (ev.events & EPOLLIN != 0) {
                revents |= EPOLLIN;
            }
            if w && (ev.events & EPOLLOUT != 0) {
                revents |= EPOLLOUT;
            }
            if e {
                revents |= EPOLLERR | EPOLLHUP;
            }
            if revents != 0 {
                out.push(EpollEvent {
                    events: revents,
                    data: ev.data,
                });
                count += 1;
            }
        }
    }
    count
}
