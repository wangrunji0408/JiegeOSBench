//! Linux native AIO for RAM files. Operations complete synchronously and are
//! queued as normal io_event records, including optional eventfd notification.
use crate::{
    fs,
    syscall::{self, buf, bytes, get32, get64, put64, Fd},
};
use alloc::{collections::VecDeque, vec::Vec};
struct Aio {
    capacity: usize,
    events: VecDeque<[u64; 4]>,
}
static mut CONTEXTS: Option<Vec<Option<Aio>>> = None;
pub unsafe fn dispatch(n: usize, a: [usize; 6]) -> isize {
    let contexts = CONTEXTS.get_or_insert_with(Vec::new);
    if n == 0 {
        if a[0] == 0 || a[0] > 4096 || get64(a[1]) != 0 {
            return -22;
        }
        contexts.push(Some(Aio {
            capacity: a[0],
            events: VecDeque::new(),
        }));
        put64(a[1], contexts.len() as u64);
        return 0;
    }
    let Some(Some(ctx)) = a[0].checked_sub(1).and_then(|id| contexts.get_mut(id)) else {
        return -22;
    };
    match n {
        1 => {
            contexts[a[0] - 1] = None;
            0
        }
        2 => {
            let mut submitted = 0;
            for i in 0..a[1] {
                if ctx.events.len() >= ctx.capacity {
                    break;
                }
                let cb = get64(a[2] + i * 8) as usize;
                let data = get64(cb);
                let opcode = core::ptr::read_unaligned((cb + 16) as *const u16);
                let fd = get32(cb + 20) as usize;
                let p = get64(cb + 24) as usize;
                let len = get64(cb + 32) as usize;
                let off = get64(cb + 40) as usize;
                let result = match syscall::get(fd) {
                    Some(Fd::File { path, .. }) => match opcode {
                        0 => {
                            if let Some(d) = fs::file_data(&path) {
                                let start = off.min(d.len());
                                let count = len.min(d.len() - start);
                                buf(p, count).copy_from_slice(&d[start..start + count]);
                                count as isize
                            } else {
                                -9
                            }
                        }
                        1 => {
                            fs::write(&path, off, bytes(p, len));
                            len as isize
                        }
                        2 | 3 => 0,
                        _ => -22,
                    },
                    _ => -9,
                };
                ctx.events.push_back([data, cb as u64, result as u64, 0]);
                if get32(cb + 56) & 1 != 0 {
                    let eventfd = get32(cb + 60) as usize;
                    if let Some(Fd::Event(v)) = syscall::get(eventfd) {
                        syscall::fds()[eventfd] = Some(Fd::Event(v + 1));
                    }
                }
                submitted += 1;
            }
            if submitted == 0 && a[1] > 0 {
                -11
            } else {
                submitted
            }
        }
        3 => -22, // Requests already completed and cannot be cancelled.
        4 => {
            let count = a[2].min(ctx.events.len());
            for i in 0..count {
                let ev = ctx.events.pop_front().unwrap();
                for j in 0..4 {
                    put64(a[3] + i * 32 + j * 8, ev[j]);
                }
            }
            count as isize
        }
        _ => -38,
    }
}
