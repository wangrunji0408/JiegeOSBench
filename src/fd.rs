use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use smoltcp::wire::IpEndpoint;
use spin::Mutex;

use crate::fs::{self, File, Metadata};
use crate::net::{EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP};

const EBADF: i32 = 9;
const EINVAL: i32 = 22;
const ESPIPE: i32 = 29;
const EAGAIN: i32 = 11;
const O_NONBLOCK: u32 = 0x800;

#[derive(Clone, Copy)]
struct SocketFd {
    id: usize,
    flags: u32,
}

#[derive(Clone, Copy)]
struct EventFd {
    counter: u64,
    flags: u32,
}

#[derive(Clone, Copy)]
struct UnixSocketFd {
    pair: usize,
    endpoint: usize,
    flags: u32,
}

struct UnixPair {
    queues: [VecDeque<u8>; 2],
    closed: [bool; 2],
}

#[derive(Clone, Copy)]
pub struct EpollInterest {
    pub events: u32,
    pub data: u64,
}

#[derive(Clone, Copy)]
pub struct EpollResult {
    pub events: u32,
    pub data: u64,
}

enum Descriptor {
    Stdin,
    Stdout,
    Stderr,
    File(File),
    Socket(SocketFd),
    EventFd(EventFd),
    UnixSocket(UnixSocketFd),
    Epoll(BTreeMap<i32, EpollInterest>),
}

static DESCRIPTORS: Mutex<Option<BTreeMap<i32, Descriptor>>> = Mutex::new(None);
static UNIX_PAIRS: Mutex<Option<BTreeMap<usize, UnixPair>>> = Mutex::new(None);
static NEXT_UNIX_PAIR: Mutex<usize> = Mutex::new(1);

pub fn init() {
    let mut descriptors = BTreeMap::new();
    descriptors.insert(0, Descriptor::Stdin);
    descriptors.insert(1, Descriptor::Stdout);
    descriptors.insert(2, Descriptor::Stderr);
    *DESCRIPTORS.lock() = Some(descriptors);
    *UNIX_PAIRS.lock() = Some(BTreeMap::new());
}

fn allocate(descriptors: &BTreeMap<i32, Descriptor>) -> i32 {
    let mut candidate = 0;
    while descriptors.contains_key(&candidate) {
        candidate += 1;
    }
    candidate
}

fn insert(descriptors: &mut BTreeMap<i32, Descriptor>, descriptor: Descriptor) -> i32 {
    let fd = allocate(descriptors);
    descriptors.insert(fd, descriptor);
    fd
}

pub fn insert_file(file: File) -> i32 {
    insert(DESCRIPTORS.lock().as_mut().unwrap(), Descriptor::File(file))
}

fn insert_socket(id: usize, flags: u32) -> i32 {
    insert(
        DESCRIPTORS.lock().as_mut().unwrap(),
        Descriptor::Socket(SocketFd { id, flags }),
    )
}

pub fn open(path: &str, flags: u32, mode: u32) -> Result<i32, i32> {
    Ok(insert_file(fs::open(path, flags, mode)?))
}

pub fn socket(domain: u32, kind: u32, _protocol: u32) -> Result<i32, i32> {
    if domain != 2 || kind & 0xf != 1 {
        return Err(EINVAL);
    }
    let id = crate::net::create()?;
    Ok(insert_socket(id, kind & O_NONBLOCK))
}

pub fn eventfd(initial: u32, flags: u32) -> i32 {
    insert(
        DESCRIPTORS.lock().as_mut().unwrap(),
        Descriptor::EventFd(EventFd {
            counter: initial as u64,
            flags,
        }),
    )
}

pub fn socketpair(domain: u32, kind: u32) -> Result<(i32, i32), i32> {
    if domain != 1 || kind & 0xf != 1 {
        return Err(EINVAL);
    }
    let pair = {
        let mut next = NEXT_UNIX_PAIR.lock();
        let pair = *next;
        *next += 1;
        pair
    };
    UNIX_PAIRS.lock().as_mut().unwrap().insert(
        pair,
        UnixPair {
            queues: [VecDeque::new(), VecDeque::new()],
            closed: [false; 2],
        },
    );
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    let flags = kind & O_NONBLOCK;
    let first = insert(
        descriptors,
        Descriptor::UnixSocket(UnixSocketFd {
            pair,
            endpoint: 0,
            flags,
        }),
    );
    let second = insert(
        descriptors,
        Descriptor::UnixSocket(UnixSocketFd {
            pair,
            endpoint: 1,
            flags,
        }),
    );
    Ok((first, second))
}

pub fn close(fd: i32) -> Result<(), i32> {
    let removed = DESCRIPTORS
        .lock()
        .as_mut()
        .unwrap()
        .remove(&fd)
        .ok_or(EBADF)?;
    match removed {
        Descriptor::Socket(socket) => crate::net::close(socket.id),
        Descriptor::UnixSocket(socket) => {
            if let Some(pair) = UNIX_PAIRS.lock().as_mut().unwrap().get_mut(&socket.pair) {
                pair.closed[socket.endpoint] = true;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn socket_id(fd: i32) -> Result<usize, i32> {
    match DESCRIPTORS
        .lock()
        .as_ref()
        .unwrap()
        .get(&fd)
        .ok_or(EBADF)?
    {
        Descriptor::Socket(socket) => Ok(socket.id),
        _ => Err(EINVAL),
    }
}

pub fn bind(fd: i32, address: [u8; 4], port: u16) -> Result<(), i32> {
    crate::net::bind(socket_id(fd)?, address, port)
}

pub fn listen(fd: i32) -> Result<(), i32> {
    crate::net::listen(socket_id(fd)?)
}

pub fn accept(fd: i32, flags: u32) -> Result<(i32, Option<IpEndpoint>), i32> {
    let (id, remote) = crate::net::accept(socket_id(fd)?)?;
    Ok((insert_socket(id, flags & O_NONBLOCK), remote))
}

pub fn shutdown(fd: i32) -> Result<(), i32> {
    crate::net::shutdown(socket_id(fd)?)
}

pub fn local_endpoint(fd: i32) -> Result<Option<IpEndpoint>, i32> {
    crate::net::local_endpoint(socket_id(fd)?)
}

pub fn remote_endpoint(fd: i32) -> Result<Option<IpEndpoint>, i32> {
    crate::net::remote_endpoint(socket_id(fd)?)
}

pub fn set_socket_option(fd: i32, level: u32, option: u32) -> Result<(), i32> {
    let id = socket_id(fd)?;
    if level == 6 && option == 1 {
        crate::net::disable_nagle(id)
    } else {
        Ok(())
    }
}

pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::Stdin => Ok(0),
        Descriptor::Stdout | Descriptor::Stderr => Err(EBADF),
        Descriptor::File(file) => file.read(buffer),
        Descriptor::Socket(socket) => crate::net::receive(socket.id, buffer),
        Descriptor::Epoll(_) => Err(EINVAL),
    }
}

pub fn write(fd: i32, buffer: &[u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::Stdin => Err(EBADF),
        Descriptor::Stdout | Descriptor::Stderr => {
            crate::console::_print(format_args!("{}", String::from_utf8_lossy(buffer)));
            Ok(buffer.len())
        }
        Descriptor::File(file) => file.write(buffer),
        Descriptor::Socket(socket) => crate::net::send(socket.id, buffer),
        Descriptor::Epoll(_) => Err(EINVAL),
    }
}

pub fn read_at(fd: i32, offset: u64, buffer: &mut [u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => {
            let previous = file.seek(0, 1)?;
            if offset > i64::MAX as u64 {
                return Err(EINVAL);
            }
            file.seek(offset as i64, 0)?;
            let result = file.read(buffer);
            file.seek(previous as i64, 0)?;
            result
        }
        _ => Err(ESPIPE),
    }
}

pub fn write_at(fd: i32, offset: u64, buffer: &[u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => {
            let previous = file.seek(0, 1)?;
            if offset > i64::MAX as u64 {
                return Err(EINVAL);
            }
            file.seek(offset as i64, 0)?;
            let result = file.write(buffer);
            file.seek(previous as i64, 0)?;
            result
        }
        _ => Err(ESPIPE),
    }
}

pub fn seek(fd: i32, offset: i64, whence: u32) -> Result<u64, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => file.seek(offset, whence),
        _ => Err(ESPIPE),
    }
}

pub fn metadata(fd: i32) -> Result<Metadata, i32> {
    let guard = DESCRIPTORS.lock();
    let descriptors = guard.as_ref().unwrap();
    match descriptors.get(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => file.metadata(),
        Descriptor::Socket(socket) => Ok(Metadata {
            mode: 0o140600,
            size: 0,
            inode: socket.id as u64,
        }),
        Descriptor::Epoll(_) => Ok(Metadata {
            mode: 0o100600,
            size: 0,
            inode: 1,
        }),
        _ => Ok(Metadata {
            mode: 0o020600,
            size: 0,
            inode: 1,
        }),
    }
}

pub fn path(fd: i32) -> Result<String, i32> {
    let guard = DESCRIPTORS.lock();
    let descriptors = guard.as_ref().unwrap();
    match descriptors.get(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => Ok(file.path().to_string()),
        _ => Err(EBADF),
    }
}

pub fn directory_entries(fd: i32) -> Result<Vec<(String, Metadata)>, i32> {
    let path = path(fd)?;
    fs::read_dir(&path)
}

pub fn directory_offset(fd: i32) -> Result<u64, i32> {
    seek(fd, 0, 1)
}

pub fn set_directory_offset(fd: i32, offset: u64) -> Result<(), i32> {
    if offset > i64::MAX as u64 {
        return Err(EINVAL);
    }
    seek(fd, offset as i64, 0).map(|_| ())
}

pub fn duplicate(fd: i32, requested: Option<i32>) -> Result<i32, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    let source = descriptors.get(&fd).ok_or(EBADF)?;
    let replacement = match source {
        Descriptor::Stdin => Descriptor::Stdin,
        Descriptor::Stdout => Descriptor::Stdout,
        Descriptor::Stderr => Descriptor::Stderr,
        _ => return Err(EINVAL),
    };
    let target = match requested {
        Some(target) => {
            if target < 0 {
                return Err(EBADF);
            }
            descriptors.remove(&target);
            target
        }
        None => allocate(descriptors),
    };
    descriptors.insert(target, replacement);
    Ok(target)
}

pub fn fcntl(fd: i32, command: u32, argument: u32) -> Result<i32, i32> {
    if command == 0 {
        return duplicate(fd, None);
    }
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    let descriptor = descriptors.get_mut(&fd).ok_or(EBADF)?;
    match command {
        1 | 2 => Ok(0),
        3 => match descriptor {
            Descriptor::Socket(socket) => Ok((socket.flags | 2) as i32),
            _ => Ok(0),
        },
        4 => {
            if let Descriptor::Socket(socket) = descriptor {
                socket.flags = argument & O_NONBLOCK;
            }
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub fn epoll_create() -> i32 {
    insert(
        DESCRIPTORS.lock().as_mut().unwrap(),
        Descriptor::Epoll(BTreeMap::new()),
    )
}

pub fn epoll_ctl(
    epoll_fd: i32,
    operation: u32,
    fd: i32,
    interest: Option<EpollInterest>,
) -> Result<(), i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    if !descriptors.contains_key(&fd) {
        return Err(EBADF);
    }
    let Descriptor::Epoll(interests) = descriptors.get_mut(&epoll_fd).ok_or(EBADF)? else {
        return Err(EINVAL);
    };
    match operation {
        1 => {
            let interest = interest.ok_or(EINVAL)?;
            if interests.insert(fd, interest).is_some() {
                return Err(17);
            }
        }
        2 => {
            interests.remove(&fd).ok_or(2)?;
        }
        3 => {
            let interest = interest.ok_or(EINVAL)?;
            let current = interests.get_mut(&fd).ok_or(2)?;
            *current = interest;
        }
        _ => return Err(EINVAL),
    }
    Ok(())
}

fn descriptor_events(fd: i32) -> Result<u32, i32> {
    let id = {
        let guard = DESCRIPTORS.lock();
        let descriptors = guard.as_ref().unwrap();
        match descriptors.get(&fd).ok_or(EBADF)? {
            Descriptor::File(_) => return Ok(EPOLLIN | EPOLLOUT),
            Descriptor::Stdout | Descriptor::Stderr => return Ok(EPOLLOUT),
            Descriptor::Stdin => return Ok(EPOLLIN),
            Descriptor::Socket(socket) => socket.id,
            Descriptor::Epoll(_) => return Ok(0),
        }
    };
    Ok(crate::net::event_flags(id))
}

pub fn epoll_wait(
    epoll_fd: i32,
    maximum: usize,
    timeout_ms: i32,
) -> Result<Vec<EpollResult>, i32> {
    let interests = {
        let guard = DESCRIPTORS.lock();
        let descriptors = guard.as_ref().unwrap();
        let Descriptor::Epoll(interests) = descriptors.get(&epoll_fd).ok_or(EBADF)? else {
            return Err(EINVAL);
        };
        interests
            .iter()
            .map(|(fd, interest)| (*fd, *interest))
            .collect::<Vec<_>>()
    };
    let start = crate::arch::time();
    let timeout_ticks = if timeout_ms < 0 {
        None
    } else {
        Some((timeout_ms as u64).saturating_mul(10_000))
    };
    loop {
        crate::net::poll();
        let mut results = Vec::new();
        for (fd, interest) in &interests {
            let available = descriptor_events(*fd)?;
            let events = available & (interest.events | EPOLLERR | EPOLLHUP | EPOLLRDHUP);
            if events != 0 {
                results.push(EpollResult {
                    events,
                    data: interest.data,
                });
                if results.len() >= maximum {
                    break;
                }
            }
        }
        let expired = timeout_ticks.is_some_and(|timeout| crate::arch::time() - start >= timeout);
        if !results.is_empty() || timeout_ms == 0 || expired {
            return Ok(results);
        }
        crate::arch::enable_interrupts();
        crate::arch::wait_for_interrupt();
        crate::arch::disable_interrupts();
    }
}
