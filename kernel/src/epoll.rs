//! epoll: level-triggered, single epoll instance per fd interest.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::{Fd, FdKind};
use crate::syscall::{read_user, write_user};
use crate::task;

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLET: u32 = 0x8000_0000;

const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

pub struct Epoll {
    pub events: Vec<(usize, u32, u64)>, // (fd, events, data)
}

pub static mut EPOLLS: Vec<Option<Epoll>> = Vec::new();
pub static mut NEXT_EPOLL: usize = 1;

fn epoll_new() -> usize {
    unsafe {
        for (i, e) in EPOLLS.iter_mut().enumerate() {
            if e.is_none() {
                *e = Some(Epoll { events: Vec::new() });
                return i;
            }
        }
        EPOLLS.push(Some(Epoll { events: Vec::new() }));
        EPOLLS.len() - 1
    }
}

pub fn wake_all_epoll() {
    // wake all tasks blocked on any epoll wchan (epoll ids are the wchan)
    let ids: Vec<usize> = unsafe {
        EPOLLS
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_some())
            .map(|(i, _)| i)
            .collect()
    };
    for id in ids {
        task::wake_wchan(id);
    }
}

pub fn sys_epoll_create1(flags: usize) -> isize {
    let _ = flags;
    let ep_id = epoll_new();
    let fds = unsafe { &mut *({
        let t = task::current();
        &mut t.as_ref().unwrap().fds as *mut _
    }) };
    let fdnum = match fds.alloc() {
        Some(fd) => fd,
        None => return -24,
    };
    fds.fds[fdnum] = Some(Fd {
        kind: FdKind::Epoll { ep_id },
        flags: 0,
        offset: 0,
        cloexec: flags & crate::fs::O_CLOEXEC != 0,
        epoll: None,
    });
    fdnum as isize
}

fn fd_kind_readable(fd: &Fd) -> bool {
    match &fd.kind {
        FdKind::Socket { sock_id } | FdKind::UnixPair { sock_id } => {
            crate::net::sock_readable(*sock_id)
        }
        FdKind::Eventfd { counter, .. } => *counter != 0,
        _ => false,
    }
}

fn fd_kind_writable(fd: &Fd) -> bool {
    match &fd.kind {
        FdKind::Socket { sock_id } | FdKind::UnixPair { sock_id } => {
            crate::net::sock_writable(*sock_id)
        }
        _ => true,
    }
}

pub fn sys_epoll_ctl(epfd: usize, op: i32, fd: usize, event: usize) -> isize {
    let ep_id = {
        let fds = unsafe { &*({
            let t = task::current();
            &t.as_ref().unwrap().fds as *const _
        }) };
        match fds.get(epfd) {
            Some(f) => match &f.kind {
                FdKind::Epoll { ep_id } => *ep_id,
                _ => return -9,
            },
            None => return -9,
        }
    };
    // read event struct: { u32 events; u64 data; } = 16 bytes
    let ev = match read_user(event, 16) {
        Ok(d) => d,
        Err(e) => return e as isize,
    };
    let events = u32::from_le_bytes(ev[..4].try_into().unwrap());
    let data = u64::from_le_bytes(ev[8..].try_into().unwrap());

    match op {
        EPOLL_CTL_ADD => {
            // register interest on the target fd
            let t = task::current();
            let fds = unsafe { &mut t.as_ref().unwrap().fds };
            let f = match fds.get_mut(fd) {
                Some(f) => f,
                None => return -9,
            };
            if f.epoll.is_some() {
                return -17; // EEXIST
            }
            f.epoll = Some((ep_id, events, data));
            let e = unsafe { EPOLLS.get_mut(ep_id).unwrap().as_mut().unwrap() };
            e.events.push((fd, events, data));
            0
        }
        EPOLL_CTL_DEL => {
            let t = task::current();
            let fds = unsafe { &mut t.as_ref().unwrap().fds };
            let f = match fds.get_mut(fd) {
                Some(f) => f,
                None => return -9,
            };
            f.epoll = None;
            let e = unsafe { EPOLLS.get_mut(ep_id).unwrap().as_mut().unwrap() };
            e.events.retain(|(f, _, _)| *f != fd);
            0
        }
        EPOLL_CTL_MOD => {
            let t = task::current();
            let fds = unsafe { &mut t.as_ref().unwrap().fds };
            let f = match fds.get_mut(fd) {
                Some(f) => f,
                None => return -9,
            };
            f.epoll = Some((ep_id, events, data));
            let e = unsafe { EPOLLS.get_mut(ep_id).unwrap().as_mut().unwrap() };
            for (f, ev, d) in e.events.iter_mut() {
                if *f == fd {
                    *ev = events;
                    *d = data;
                }
            }
            0
        }
        _ => -22,
    }
}

/// Called when an fd is closed: drop its epoll registration.
pub fn fd_removed(fd: usize) {
    let t = task::current();
    // look up old registration before removal (fds already closed here)
    // We scan all epolls for the fd instead (registration lives in both places).
    unsafe {
        for e in EPOLLS.iter_mut() {
            if let Some(e) = e {
                e.events.retain(|(f, _, _)| *f != fd);
            }
        }
    }
}

pub fn sys_epoll_pwait(epfd: usize, events: usize, maxevents: usize, timeout_ms: isize, _sigmask: usize) -> isize {
    let ep_id = {
        let fds = unsafe { &*({
            let t = task::current();
            &t.as_ref().unwrap().fds as *const _
        }) };
        match fds.get(epfd) {
            Some(f) => match &f.kind {
                FdKind::Epoll { ep_id } => *ep_id,
                _ => return -9,
            },
            None => return -9,
        }
    };
    if maxevents == 0 {
        return -22;
    }
    let deadline = if timeout_ms < 0 {
        None
    } else {
        Some(crate::timer::now_ms() + timeout_ms as u64)
    };
    loop {
        // level-triggered scan
        let mut ready: Vec<(u32, u64)> = Vec::new();
        let (events_list, fds_snapshot) = {
            let t = task::current();
            let fds = unsafe { &t.as_ref().unwrap().fds };
            let e = unsafe { EPOLLS.get(ep_id).unwrap().as_ref().unwrap() };
            let evs = e.events.clone();
            let mut snap = Vec::new();
            for (fd, _, _) in &evs {
                snap.push((*fd, fds.get(*fd).cloned()));
            }
            (evs, snap)
        };
        for (fd, want, data) in &events_list {
            if let Some((_, f)) = fds_snapshot.iter().find(|(f, _)| *f == *fd) {
                if let Some(f) = f {
                    let mut rev = 0u32;
                    if fd_kind_readable(f) {
                        rev |= EPOLLIN;
                        if matches!(
                            f.kind,
                            FdKind::Socket { .. } | FdKind::UnixPair { .. }
                        ) && {
                            let sid = match f.kind {
                                FdKind::Socket { sock_id } | FdKind::UnixPair { sock_id } => sock_id,
                                _ => 0,
                            };
                            crate::net::sock(sid).map(|s| s.peer_fin).unwrap_or(false)
                        } {
                            rev |= EPOLLRDHUP;
                        }
                    }
                    if fd_kind_writable(f) {
                        rev |= EPOLLOUT;
                    }
                    rev &= want;
                    if rev != 0 {
                        ready.push((rev, *data));
                    }
                }
            }
        }
        if !ready.is_empty() {
            // write results
            let n = core::cmp::min(ready.len(), maxevents);
            for i in 0..n {
                let mut ev = [0u8; 16];
                ev[..4].copy_from_slice(&ready[i].0.to_le_bytes());
                ev[8..].copy_from_slice(&ready[i].1.to_le_bytes());
                if write_user(events + i * 16, &ev).is_err() {
                    return -14;
                }
            }
            return n as isize;
        }
        // nothing ready: block or timeout
        if let Some(dl) = deadline {
            if crate::timer::now_ms() >= dl {
                return 0;
            }
            crate::timer_wheel::set_timer(dl, ep_id, crate::timer_wheel::TimerKind::Wake);
        }
        if crate::signal::has_pending() {
            return -4; // EINTR
        }
        task::block_on(ep_id);
        // woken: check signal, re-scan
        if crate::signal::has_pending() {
            return -4;
        }
    }
}

// keep BTreeMap import used (future)
pub fn _btree_guard() {
    let _ = BTreeMap::<usize, usize>::new();
}
