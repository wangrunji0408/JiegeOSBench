use crate::fs::epoll::{EpollEntry, EpollFile};
use crate::fs::eventfd::EventFd;
use crate::mm::translated_byte_buffer;
use crate::task::{current_task, current_user_token, suspend_current_and_run_next};
use alloc::sync::Arc;

const EPOLL_CTL_ADD: usize = 1;
const EPOLL_CTL_DEL: usize = 2;
const EPOLL_CTL_MOD: usize = 3;

pub fn sys_epoll_create1(_flags: usize) -> isize {
    let task = current_task().unwrap();
    let fd = task.inner_lock().alloc_fd(Arc::new(EpollFile::new()));
    fd as isize
}

fn read_epoll_event(token: usize, ptr: *const u8) -> (u32, u64) {
    let chunks = translated_byte_buffer(token, ptr, 16);
    let mut raw = [0u8; 16];
    let mut off = 0;
    for c in chunks {
        raw[off..off + c.len()].copy_from_slice(c);
        off += c.len();
    }
    let events = u32::from_ne_bytes(raw[0..4].try_into().unwrap());
    let data = u64::from_ne_bytes(raw[8..16].try_into().unwrap());
    (events, data)
}

fn write_epoll_event(token: usize, ptr: *mut u8, events: u32, data: u64) {
    let mut raw = [0u8; 16];
    raw[0..4].copy_from_slice(&events.to_ne_bytes());
    raw[8..16].copy_from_slice(&data.to_ne_bytes());
    let mut chunks = translated_byte_buffer(token, ptr, 16);
    let mut copied = 0;
    for c in chunks.iter_mut() {
        let n = c.len();
        c.copy_from_slice(&raw[copied..copied + n]);
        copied += n;
    }
}

pub fn sys_epoll_ctl(epfd: usize, op: usize, fd: usize, event_ptr: *const u8) -> isize {
    let task = current_task().unwrap();
    let epoll_file = match task.inner_lock().get_fd(epfd) {
        Some(f) => f,
        None => return -9,
    };
    let Some(epoll) = epoll_file.as_any().downcast_ref::<EpollFile>() else {
        return -22;
    };
    let target = match task.inner_lock().get_fd(fd) {
        Some(f) => f,
        None => return -9,
    };
    let mut entries = epoll.entries.lock();
    match op {
        EPOLL_CTL_ADD => {
            let token = current_user_token();
            let (events, data) = read_epoll_event(token, event_ptr);
            entries.retain(|e| e.fd != fd as i32);
            entries.push(EpollEntry {
                fd: fd as i32,
                events,
                data,
                file: target,
            });
            0
        }
        EPOLL_CTL_MOD => {
            let token = current_user_token();
            let (events, data) = read_epoll_event(token, event_ptr);
            if let Some(e) = entries.iter_mut().find(|e| e.fd == fd as i32) {
                e.events = events;
                e.data = data;
            }
            0
        }
        EPOLL_CTL_DEL => {
            entries.retain(|e| e.fd != fd as i32);
            0
        }
        _ => -22,
    }
}

const EPOLLIN: u32 = 0x1;
const EPOLLOUT: u32 = 0x4;

pub fn sys_epoll_pwait(epfd: usize, events_ptr: *mut u8, maxevents: usize, timeout_ms: isize) -> isize {
    let task = current_task().unwrap();
    let epoll_file = match task.inner_lock().get_fd(epfd) {
        Some(f) => f,
        None => return -9,
    };
    let Some(epoll) = epoll_file.as_any().downcast_ref::<EpollFile>() else {
        return -22;
    };
    let token = current_user_token();
    let start = riscv::register::time::read64();
    let timeout_ticks = if timeout_ms < 0 {
        u64::MAX
    } else {
        (timeout_ms as u64) * 10_000
    };
    loop {
        crate::net::poll();
        let mut out_ptr = events_ptr;
        let mut count = 0usize;
        {
            let entries = epoll.entries.lock();
            for e in entries.iter() {
                if count >= maxevents {
                    break;
                }
                let mut revents = 0u32;
                if e.events & EPOLLIN != 0 && e.file.poll_readable() {
                    revents |= EPOLLIN;
                }
                if e.events & EPOLLOUT != 0 && e.file.poll_writable() {
                    revents |= EPOLLOUT;
                }
                if revents != 0 {
                    write_epoll_event(token, out_ptr, revents, e.data);
                    unsafe {
                        out_ptr = out_ptr.add(16);
                    }
                    count += 1;
                }
            }
        }
        if count > 0 {
            crate::println!(
                "[dbg] epoll_pwait(pid={}) -> {} events ready",
                crate::task::current_pid(),
                count
            );
            return count as isize;
        }
        if riscv::register::time::read64().saturating_sub(start) >= timeout_ticks {
            return 0;
        }
        suspend_current_and_run_next();
    }
}

pub fn sys_eventfd2(initval: usize, flags: usize) -> isize {
    const EFD_SEMAPHORE: usize = 1;
    let file = Arc::new(EventFd::new(initval as u64, flags & EFD_SEMAPHORE != 0));
    let task = current_task().unwrap();
    let fd = task.inner_lock().alloc_fd(file);
    fd as isize
}
