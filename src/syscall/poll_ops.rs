//! `poll`, `select`, `epoll`, and `eventfd`.
//!
//! nginx's default event method on Linux is epoll, so `epoll_create1`,
//! `epoll_ctl` and `epoll_wait` carry the entire request-serving loop.

use crate::fs::inode::{next_ino, Inode, InodeKind};
use crate::fs::stat::Timespec;
use crate::fs::{File, OpenFlags, Result};
use crate::mm::uaccess;
use crate::{bail, impl_as_any, task};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Poll event bits.
pub const POLLIN: u16 = 0x001;
pub const POLLPRI: u16 = 0x002;
pub const POLLOUT: u16 = 0x004;
pub const POLLERR: u16 = 0x008;
pub const POLLHUP: u16 = 0x010;
pub const POLLNVAL: u16 = 0x020;
pub const POLLRDNORM: u16 = 0x040;
pub const POLLRDHUP: u16 = 0x2000;

/// `struct pollfd`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PollFd {
    fd: i32,
    events: u16,
    revents: u16,
}

/// Evaluate the readiness of one descriptor against a requested event mask.
fn check_events(file: &Arc<File>, requested: u16) -> u16 {
    let mut revents = 0u16;
    if requested & (POLLIN | POLLRDNORM) != 0 && file.poll_readable() {
        revents |= POLLIN & requested | POLLRDNORM & requested;
        // Report POLLIN even if only POLLRDNORM was asked for, as Linux does.
        if revents == 0 {
            revents |= POLLIN;
        }
    }
    if requested & POLLOUT != 0 && file.poll_writable() {
        revents |= POLLOUT;
    }
    // Error and hangup are always reported, regardless of the request mask.
    if file.poll_error() {
        revents |= POLLERR;
    }
    if file.poll_hangup() {
        revents |= POLLHUP;
        if requested & POLLRDHUP != 0 {
            revents |= POLLRDHUP;
        }
    }
    revents
}

/// Convert a `timespec` timeout into an absolute deadline in ms.
fn deadline_from_timespec(ptr: usize) -> Result<Option<u64>> {
    if ptr == 0 {
        return Ok(None);
    }
    let ts: Timespec = uaccess::read(ptr)?;
    if ts.sec < 0 || ts.nsec < 0 {
        bail!(EINVAL);
    }
    let ms = (ts.sec as u64) * 1000 + (ts.nsec as u64 + 999_999) / 1_000_000;
    Ok(Some(crate::time::monotonic_ms() + ms))
}

pub fn sys_ppoll(fds_ptr: usize, nfds: usize, timeout_ptr: usize, _sigmask: usize) -> Result<isize> {
    if nfds > 4096 {
        bail!(EINVAL);
    }
    let deadline = deadline_from_timespec(timeout_ptr)?;

    // Read the request array once.
    let mut requests: Vec<PollFd> = Vec::with_capacity(nfds);
    for i in 0..nfds {
        requests.push(uaccess::read(fds_ptr + i * core::mem::size_of::<PollFd>())?);
    }

    let task = task::current();
    loop {
        let mut ready = 0;
        for request in requests.iter_mut() {
            request.revents = 0;
            if request.fd < 0 {
                continue;
            }
            match task.files.lock().get(request.fd) {
                Some(file) => {
                    request.revents = check_events(&file, request.events);
                }
                None => request.revents = POLLNVAL,
            }
            if request.revents != 0 {
                ready += 1;
            }
        }

        if ready > 0 {
            for (i, request) in requests.iter().enumerate() {
                uaccess::write(fds_ptr + i * core::mem::size_of::<PollFd>(), *request)?;
            }
            return Ok(ready as isize);
        }

        // Timed out?
        if let Some(deadline) = deadline {
            if crate::time::monotonic_ms() >= deadline {
                // Write back the (all-zero) revents.
                for (i, request) in requests.iter().enumerate() {
                    uaccess::write(fds_ptr + i * core::mem::size_of::<PollFd>(), *request)?;
                }
                return Ok(0);
            }
        }
        if task::has_pending_signal() {
            bail!(EINTR);
        }
        crate::net::poll();
        task::yield_now();
    }
}

/// An `fd_set`: 1024 bits.
const FD_SETSIZE: usize = 1024;
const FD_SET_WORDS: usize = FD_SETSIZE / 64;

fn read_fdset(ptr: usize, nfds: usize) -> Result<[u64; FD_SET_WORDS]> {
    let mut set = [0u64; FD_SET_WORDS];
    if ptr == 0 {
        return Ok(set);
    }
    let words = (nfds + 63) / 64;
    for i in 0..words.min(FD_SET_WORDS) {
        set[i] = uaccess::read(ptr + i * 8)?;
    }
    Ok(set)
}

fn write_fdset(ptr: usize, set: &[u64; FD_SET_WORDS], nfds: usize) -> Result<()> {
    if ptr == 0 {
        return Ok(());
    }
    let words = (nfds + 63) / 64;
    for i in 0..words.min(FD_SET_WORDS) {
        uaccess::write(ptr + i * 8, set[i])?;
    }
    Ok(())
}

#[inline]
fn fd_isset(set: &[u64; FD_SET_WORDS], fd: usize) -> bool {
    fd < FD_SETSIZE && set[fd / 64] & (1 << (fd % 64)) != 0
}

#[inline]
fn fd_set(set: &mut [u64; FD_SET_WORDS], fd: usize) {
    if fd < FD_SETSIZE {
        set[fd / 64] |= 1 << (fd % 64);
    }
}

pub fn sys_pselect6(
    nfds: i32,
    readfds: usize,
    writefds: usize,
    exceptfds: usize,
    timeout_ptr: usize,
    _sigmask: usize,
) -> Result<isize> {
    if nfds < 0 || nfds as usize > FD_SETSIZE {
        bail!(EINVAL);
    }
    let nfds = nfds as usize;
    let deadline = deadline_from_timespec(timeout_ptr)?;

    let want_read = read_fdset(readfds, nfds)?;
    let want_write = read_fdset(writefds, nfds)?;
    let want_except = read_fdset(exceptfds, nfds)?;

    let task = task::current();
    loop {
        let mut got_read = [0u64; FD_SET_WORDS];
        let mut got_write = [0u64; FD_SET_WORDS];
        let mut got_except = [0u64; FD_SET_WORDS];
        let mut ready = 0;

        for fd in 0..nfds {
            let in_read = fd_isset(&want_read, fd);
            let in_write = fd_isset(&want_write, fd);
            let in_except = fd_isset(&want_except, fd);
            if !in_read && !in_write && !in_except {
                continue;
            }
            let Some(file) = task.files.lock().get(fd as i32) else {
                bail!(EBADF);
            };
            if in_read && (file.poll_readable() || file.poll_hangup()) {
                fd_set(&mut got_read, fd);
                ready += 1;
            }
            if in_write && file.poll_writable() {
                fd_set(&mut got_write, fd);
                ready += 1;
            }
            if in_except && file.poll_error() {
                fd_set(&mut got_except, fd);
                ready += 1;
            }
        }

        if ready > 0 || deadline.is_some_and(|d| crate::time::monotonic_ms() >= d) {
            write_fdset(readfds, &got_read, nfds)?;
            write_fdset(writefds, &got_write, nfds)?;
            write_fdset(exceptfds, &got_except, nfds)?;
            return Ok(ready as isize);
        }
        if task::has_pending_signal() {
            bail!(EINTR);
        }
        crate::net::poll();
        task::yield_now();
    }
}

// ---------------------------------------------------------------------------
// epoll
// ---------------------------------------------------------------------------

/// `struct epoll_event`.
///
/// Only x86 packs this struct; on riscv64 (and every other architecture) it is
/// naturally aligned, so `data` sits at offset 8 and the whole thing is 16 bytes.
/// Getting this wrong truncates every pointer nginx stores in `data`, which
/// shows up as a segfault deep in its event loop rather than as a bad syscall.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    _pad: u32,
    data: u64,
}

const EPOLL_CTL_ADD: u32 = 1;
const EPOLL_CTL_DEL: u32 = 2;
const EPOLL_CTL_MOD: u32 = 3;

const EPOLLIN: u32 = 0x001;
const EPOLLPRI: u32 = 0x002;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLRDNORM: u32 = 0x040;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLEXCLUSIVE: u32 = 1 << 28;
const EPOLLET: u32 = 1 << 31;
const EPOLLONESHOT: u32 = 1 << 30;

/// One watched descriptor.
#[derive(Clone)]
struct EpollEntry {
    events: u32,
    data: u64,
    /// For edge-triggered watches: the readiness we last reported, so we only
    /// report the transition.
    last_reported: u32,
    /// The open file description this watch was registered against.
    ///
    /// Linux keys epoll registrations on the *description*, not the descriptor
    /// number, and drops a registration automatically when its last descriptor
    /// closes. Without this, a recycled fd number inherits the stale watch and
    /// `EPOLL_CTL_ADD` fails with `EEXIST` — which nginx reports as
    /// "epoll_ctl(1, 3) failed (17: File exists)" and then drops the connection.
    /// A weak reference lets us notice the description is gone.
    file: alloc::sync::Weak<File>,
}

impl EpollEntry {
    /// Is this registration still valid for `fd`?
    ///
    /// It is stale if the description was freed, or if the descriptor now refers
    /// to a different description than the one we registered.
    fn matches(&self, current: &Arc<File>) -> bool {
        match self.file.upgrade() {
            Some(registered) => Arc::ptr_eq(&registered, current),
            None => false,
        }
    }
}

/// An epoll instance, exposed to user space as a file descriptor.
pub struct EpollInstance {
    ino: u64,
    entries: Mutex<BTreeMap<i32, EpollEntry>>,
}

impl EpollInstance {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            entries: Mutex::new(BTreeMap::new()),
        })
    }

    /// Gather ready events.
    fn collect(&self, max: usize) -> Vec<EpollEvent> {
        let task = task::current();
        let mut out = Vec::new();
        let mut entries = self.entries.lock();
        let mut stale: Vec<i32> = Vec::new();

        for (&fd, entry) in entries.iter_mut() {
            if out.len() >= max {
                break;
            }
            let Some(file) = task.files.lock().get(fd) else {
                // The descriptor was closed without EPOLL_CTL_DEL; Linux drops
                // it silently.
                stale.push(fd);
                continue;
            };
            // The fd number was recycled onto a different open file: our watch
            // died with the old description.
            if !entry.matches(&file) {
                stale.push(fd);
                continue;
            }

            let mut ready = 0u32;
            if entry.events & (EPOLLIN | EPOLLRDNORM) != 0 && file.poll_readable() {
                ready |= EPOLLIN;
                if entry.events & EPOLLRDNORM != 0 {
                    ready |= EPOLLRDNORM;
                }
            }
            if entry.events & EPOLLOUT != 0 && file.poll_writable() {
                ready |= EPOLLOUT;
            }
            if file.poll_error() {
                ready |= EPOLLERR;
            }
            if file.poll_hangup() {
                ready |= EPOLLHUP;
                if entry.events & EPOLLRDHUP != 0 {
                    ready |= EPOLLRDHUP;
                }
            }

            // Edge-triggered: report only the bits that have newly become set.
            //
            // Per-bit tracking is what makes this correct. `EPOLLHUP` latches on
            // for the rest of a half-closed connection's life, and a writable
            // socket keeps `EPOLLOUT` set; if the comparison were on the whole
            // mask, one sticky bit would keep `last_reported` non-zero and make
            // every later `EPOLLIN` arrival look already-reported. nginx would
            // then never learn about the next request on a keep-alive connection:
            // the kernel ACKs the bytes, the worker sits in `epoll_wait`, and the
            // client times out.
            if entry.events & EPOLLET != 0 {
                let fresh = ready & !entry.last_reported;
                // Remember exactly what is set now, so a bit that clears is
                // eligible to fire again the moment it returns.
                entry.last_reported = ready;
                if fresh == 0 {
                    continue;
                }
            } else if ready == 0 {
                continue;
            }

            crate::trace!(
                "epoll: fd={} reporting {:#x} (watching {:#x})",
                fd,
                ready,
                entry.events
            );

            out.push(EpollEvent {
                events: ready,
                _pad: 0,
                data: entry.data,
            });

            // EPOLLONESHOT: disable the watch after reporting.
            if entry.events & EPOLLONESHOT != 0 {
                entry.events &= !(EPOLLIN | EPOLLOUT | EPOLLRDNORM | EPOLLRDHUP | EPOLLPRI);
            }
        }
        for fd in stale {
            entries.remove(&fd);
        }
        out
    }
}

impl Inode for EpollInstance {
    fn kind(&self) -> InodeKind {
        // Linux reports an anonymous inode; a regular file is close enough for
        // anything that fstats an epoll fd.
        InodeKind::File
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o600
    }
    fn poll_readable(&self) -> bool {
        !self.collect(1).is_empty()
    }
    fn poll_writable(&self) -> bool {
        false
    }
    impl_as_any!();
}

pub fn sys_epoll_create1(flags: u32) -> Result<isize> {
    let instance = EpollInstance::new();
    let file = Arc::new(File::with_path(
        instance,
        OpenFlags::RDWR,
        "anon_inode:[eventpoll]",
    ));
    // EPOLL_CLOEXEC has the same value as O_CLOEXEC.
    let cloexec = flags & OpenFlags::CLOEXEC.bits() != 0;
    let fd = task::current().files.lock().insert(file, cloexec)?;
    Ok(fd as isize)
}

pub fn sys_epoll_ctl(epfd: i32, op: u32, fd: i32, event_ptr: usize) -> Result<isize> {
    let task = task::current();
    let ep_file = task.files.lock().get_or_err(epfd)?;
    let instance = ep_file
        .inode
        .as_any()
        .downcast_ref::<EpollInstance>()
        .ok_or(crate::err!(EINVAL))?;

    if epfd == fd {
        bail!(EINVAL);
    }
    // The target must be a valid descriptor.
    let target = task.files.lock().get_or_err(fd)?;

    match op {
        EPOLL_CTL_ADD => {
            let event: EpollEvent = uaccess::read(event_ptr)?;
            let mut entries = instance.entries.lock();
            // Only an entry still pointing at *this* description conflicts. A
            // leftover from a closed fd that happened to reuse this number does
            // not: Linux would have dropped it when the description died.
            if let Some(existing) = entries.get(&fd) {
                if existing.matches(&target) {
                    bail!(EEXIST);
                }
            }
            crate::trace!(
                "epoll_ctl ADD fd={} events={:#x}{}",
                fd,
                event.events,
                if event.events & EPOLLET != 0 { " [ET]" } else { "" }
            );
            entries.insert(
                fd,
                EpollEntry {
                    events: event.events,
                    data: event.data,
                    last_reported: 0,
                    file: Arc::downgrade(&target),
                },
            );
            Ok(0)
        }
        EPOLL_CTL_MOD => {
            let event: EpollEvent = uaccess::read(event_ptr)?;
            let mut entries = instance.entries.lock();
            let entry = entries.get_mut(&fd).ok_or(crate::err!(ENOENT))?;
            if !entry.matches(&target) {
                bail!(ENOENT);
            }
            entry.events = event.events;
            entry.data = event.data;
            // Re-arm edge-triggered reporting.
            entry.last_reported = 0;
            Ok(0)
        }
        EPOLL_CTL_DEL => {
            let mut entries = instance.entries.lock();
            entries.remove(&fd).ok_or(crate::err!(ENOENT))?;
            Ok(0)
        }
        _ => bail!(EINVAL),
    }
}

pub fn sys_epoll_pwait(
    epfd: i32,
    events_ptr: usize,
    max_events: i32,
    timeout_ms: i64,
    _sigmask: usize,
) -> Result<isize> {
    if max_events <= 0 {
        bail!(EINVAL);
    }
    let deadline = if timeout_ms < 0 {
        None
    } else {
        Some(crate::time::monotonic_ms() + timeout_ms as u64)
    };
    epoll_wait_inner(epfd, events_ptr, max_events as usize, deadline)
}

pub fn sys_epoll_pwait2(
    epfd: i32,
    events_ptr: usize,
    max_events: i32,
    timeout_ptr: usize,
) -> Result<isize> {
    if max_events <= 0 {
        bail!(EINVAL);
    }
    let deadline = deadline_from_timespec(timeout_ptr)?;
    epoll_wait_inner(epfd, events_ptr, max_events as usize, deadline)
}

fn epoll_wait_inner(
    epfd: i32,
    events_ptr: usize,
    max_events: usize,
    deadline: Option<u64>,
) -> Result<isize> {
    let task = task::current();
    let ep_file = task.files.lock().get_or_err(epfd)?;
    // Hold an owning reference to the instance so we can drop the file lock.
    let instance = ep_file
        .inode
        .clone()
        .as_any()
        .downcast_ref::<EpollInstance>()
        .map(|_| ep_file.inode.clone())
        .ok_or(crate::err!(EINVAL))?;
    let instance = instance
        .as_any()
        .downcast_ref::<EpollInstance>()
        .ok_or(crate::err!(EINVAL))?;

    loop {
        crate::net::poll();
        let ready = instance.collect(max_events);
        if !ready.is_empty() {
            for (i, event) in ready.iter().enumerate() {
                uaccess::write(events_ptr + i * core::mem::size_of::<EpollEvent>(), *event)?;
            }
            return Ok(ready.len() as isize);
        }

        if let Some(deadline) = deadline {
            if crate::time::monotonic_ms() >= deadline {
                return Ok(0);
            }
        }
        if task::has_pending_signal() {
            bail!(EINTR);
        }
        task::yield_now();
    }
}

// ---------------------------------------------------------------------------
// eventfd
// ---------------------------------------------------------------------------

/// An eventfd: a 64-bit counter that reads drain and writes increment.
pub struct EventFd {
    ino: u64,
    counter: AtomicU64,
    /// EFD_SEMAPHORE: reads decrement by one instead of draining.
    semaphore: AtomicBool,
}

const EFD_SEMAPHORE: u32 = 1;
const EFD_NONBLOCK: u32 = 0o4000;
const EFD_CLOEXEC: u32 = 0o2000;

impl Inode for EventFd {
    fn kind(&self) -> InodeKind {
        InodeKind::File
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o600
    }

    fn read(&self, _offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        if buf.len() < 8 {
            bail!(EINVAL);
        }
        loop {
            let value = self.counter.load(Ordering::Acquire);
            if value > 0 {
                let taken = if self.semaphore.load(Ordering::Relaxed) {
                    self.counter.fetch_sub(1, Ordering::AcqRel);
                    1
                } else {
                    self.counter.store(0, Ordering::Release);
                    value
                };
                buf[..8].copy_from_slice(&taken.to_ne_bytes());
                return Ok(8);
            }
            if nonblock {
                bail!(EAGAIN);
            }
            task::yield_now();
            if task::has_pending_signal() {
                bail!(EINTR);
            }
        }
    }

    fn write(&self, _offset: usize, buf: &[u8], _nonblock: bool) -> Result<usize> {
        if buf.len() < 8 {
            bail!(EINVAL);
        }
        let value = u64::from_ne_bytes(buf[..8].try_into().unwrap());
        // u64::MAX is reserved.
        if value == u64::MAX {
            bail!(EINVAL);
        }
        self.counter.fetch_add(value, Ordering::AcqRel);
        Ok(8)
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.read(0, buf, true)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        self.write(0, buf, true)
    }

    fn poll_readable(&self) -> bool {
        self.counter.load(Ordering::Relaxed) > 0
    }

    fn poll_writable(&self) -> bool {
        self.counter.load(Ordering::Relaxed) < u64::MAX - 1
    }

    impl_as_any!();
}

pub fn sys_eventfd2(initial: u32, flags: u32) -> Result<isize> {
    let efd = Arc::new(EventFd {
        ino: next_ino(),
        counter: AtomicU64::new(initial as u64),
        semaphore: AtomicBool::new(flags & EFD_SEMAPHORE != 0),
    });
    let mut open_flags = OpenFlags::RDWR;
    if flags & EFD_NONBLOCK != 0 {
        open_flags |= OpenFlags::NONBLOCK;
    }
    let file = Arc::new(File::with_path(efd, open_flags, "anon_inode:[eventfd]"));
    let cloexec = flags & EFD_CLOEXEC != 0;
    let fd = task::current().files.lock().insert(file, cloexec)?;
    Ok(fd as isize)
}

/// Keep the exclusive flag documented; we treat every watch as exclusive since
/// only one epoll instance can be interested in a descriptor in our model.
const _: u32 = EPOLLEXCLUSIVE | POLLPRI as u32;
