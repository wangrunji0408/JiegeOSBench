/// 时间相关syscall

use crate::task::{current_task, manager::TASK_MANAGER};
use crate::task::process::TaskState;

use super::*;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i64,
}

pub fn sys_nanosleep(req_va: usize, rem_va: usize) -> isize {
    let task = current_task().unwrap();
    let (sec, nsec) = {
        let t = task.lock();
        let mut buf = [0u8; 16];
        t.memory_set.copy_from_user(req_va, &mut buf);
        let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
        let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
        (sec, nsec)
    };

    let sleep_ms = sec as usize * 1000 + nsec as usize / 1_000_000;
    let until = crate::timer::get_time_ms() + sleep_ms;

    {
        let pid = task.lock().pid;
        let mut mgr = TASK_MANAGER.lock();
        if let Some(t) = mgr.tasks.get(&pid) {
            t.lock().state = TaskState::Sleeping(until);
        }
    }

    crate::task::schedule();

    0
}

pub fn sys_clock_gettime(clockid: i32, tp_va: usize) -> isize {
    let (sec, nsec) = {
        let ms = crate::timer::get_time_ms();
        let us = crate::timer::get_time_us();
        (ms / 1000, (us % 1_000_000) * 1000)
    };

    let ts = Timespec {
        tv_sec: sec as i64,
        tv_nsec: nsec as i64,
    };

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(tp_va, bytemuck_cast(core::slice::from_ref(&ts)));
    0
}

pub fn sys_gettimeofday(tv_va: usize, tz_va: usize) -> isize {
    let us = crate::timer::get_time_us();
    let tv = Timeval {
        tv_sec: (us / 1_000_000) as i64,
        tv_usec: (us % 1_000_000) as i64,
    };

    let task = current_task().unwrap();
    let t = task.lock();
    t.memory_set.copy_to_user(tv_va, bytemuck_cast(core::slice::from_ref(&tv)));
    0
}

pub fn sys_times(buf_va: usize) -> isize {
    if buf_va != 0 {
        let task = current_task().unwrap();
        let t = task.lock();
        let tms = [0u64; 4]; // utime, stime, cutime, cstime
        t.memory_set.copy_to_user(buf_va, bytemuck_cast(&tms));
    }
    crate::timer::get_time_ms() as isize
}

fn bytemuck_cast<T>(s: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(
            s.as_ptr() as *const u8,
            s.len() * core::mem::size_of::<T>(),
        )
    }
}
