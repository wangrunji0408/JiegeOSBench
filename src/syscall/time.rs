//! Time-related syscalls.
use super::*;

const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;

pub fn clock_gettime(clk: usize, tp: usize) -> SysResult {
    let ns = match clk {
        CLOCK_REALTIME => crate::time::unix_ns(),
        CLOCK_MONOTONIC | _ => crate::time::now_ns(),
    };
    write_user(tp, (ns / 1_000_000_000) as i64)?;
    write_user(tp + 8, (ns % 1_000_000_000) as i64)?;
    Ok(0)
}

pub fn clock_getres(_clk: usize, tp: usize) -> SysResult {
    if tp != 0 {
        write_user(tp, 0i64)?;
        write_user(tp + 8, 100i64)?;
    }
    Ok(0)
}

pub fn gettimeofday(tv: usize, _tz: usize) -> SysResult {
    if tv != 0 {
        let ns = crate::time::unix_ns();
        write_user(tv, (ns / 1_000_000_000) as i64)?;
        write_user(tv + 8, ((ns % 1_000_000_000) / 1000) as i64)?;
    }
    Ok(0)
}

pub fn nanosleep(req: usize, _rem: usize) -> SysResult {
    let sec: i64 = read_user(req)?;
    let nsec: i64 = read_user(req + 8)?;
    crate::time::spin_sleep_ns(sec as u64 * 1_000_000_000 + nsec as u64);
    Ok(0)
}

pub fn clock_nanosleep(_clk: usize, flags: usize, req: usize, _rem: usize) -> SysResult {
    let sec: i64 = read_user(req)?;
    let nsec: i64 = read_user(req + 8)?;
    let ns = sec as u64 * 1_000_000_000 + nsec as u64;
    if flags & 1 != 0 {
        // TIMER_ABSTIME
        let now = crate::time::now_ns();
        if ns > now {
            crate::time::spin_sleep_ns(ns - now);
        }
    } else {
        crate::time::spin_sleep_ns(ns);
    }
    Ok(0)
}
