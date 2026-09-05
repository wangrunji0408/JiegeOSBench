//! Time and miscellaneous system calls.
use crate::abi::*;
use crate::mm::uaccess::*;
use crate::task::current;
use crate::time::{monotonic_ns, realtime_ns};

fn ts_from_ns(ns: u64) -> Timespec {
    Timespec { tv_sec: (ns / 1_000_000_000) as i64, tv_nsec: (ns % 1_000_000_000) as i64 }
}

pub fn sys_clock_gettime(clk: i32, tp: usize) -> SysResult {
    let ns = match clk {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => realtime_ns(),
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME => monotonic_ns(),
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            let c = current();
            (c.utime.load(core::sync::atomic::Ordering::Relaxed) + c.stime.load(core::sync::atomic::Ordering::Relaxed))
                as u64
        }
        _ => return Err(EINVAL),
    };
    write_val(tp, ts_from_ns(ns))?;
    Ok(0)
}

pub fn sys_clock_getres(_clk: i32, tp: usize) -> SysResult {
    if tp != 0 {
        write_val(tp, Timespec { tv_sec: 0, tv_nsec: 100 })?;
    }
    Ok(0)
}

pub fn sys_clock_settime(clk: i32, tp: usize) -> SysResult {
    if clk != CLOCK_REALTIME {
        return Err(EINVAL);
    }
    let ts: Timespec = read_val(tp)?;
    crate::time::set_realtime_ns(ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64);
    Ok(0)
}

pub fn sys_gettimeofday(tv: usize, _tz: usize) -> SysResult {
    if tv != 0 {
        let ns = realtime_ns();
        write_val(tv, Timeval { tv_sec: (ns / 1_000_000_000) as i64, tv_usec: ((ns % 1_000_000_000) / 1000) as i64 })?;
    }
    Ok(0)
}

fn do_sleep(deadline: u64, rem: usize) -> SysResult {
    match crate::time::sleep_until(deadline) {
        Ok(()) => Ok(0),
        Err(remaining) => {
            if rem != 0 {
                write_val(rem, ts_from_ns(remaining))?;
            }
            Err(EINTR)
        }
    }
}

pub fn sys_nanosleep(req: usize, rem: usize) -> SysResult {
    let ts: Timespec = read_val(req)?;
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let deadline = monotonic_ns() + ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    do_sleep(deadline, rem)
}

pub fn sys_clock_nanosleep(clk: i32, flags: i32, req: usize, rem: usize) -> SysResult {
    let ts: Timespec = read_val(req)?;
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let dur = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    let deadline = if flags & 1 != 0 {
        // TIMER_ABSTIME
        match clk {
            CLOCK_REALTIME => {
                let now = realtime_ns();
                monotonic_ns() + dur.saturating_sub(now)
            }
            _ => dur,
        }
    } else {
        monotonic_ns() + dur
    };
    do_sleep(deadline, if flags & 1 != 0 { 0 } else { rem })
}

pub fn sys_setitimer(which: i32, new: usize, old: usize) -> SysResult {
    if which != 0 {
        return Err(EINVAL);
    }
    let cur = current();
    if old != 0 {
        write_val(old, [0u64; 4])?;
    }
    if new != 0 {
        let v: [Timeval; 2] = read_val(new)?;
        let interval = v[0].tv_sec as u64 * 1_000_000_000 + v[0].tv_usec as u64 * 1000;
        let value = v[1].tv_sec as u64 * 1_000_000_000 + v[1].tv_usec as u64 * 1000;
        crate::time::set_itimer(&cur, value, interval);
    }
    Ok(0)
}

pub fn sys_getitimer(_which: i32, cur: usize) -> SysResult {
    write_val(cur, [0u64; 4])?;
    Ok(0)
}

fn fill(dst: &mut [u8; 65], s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(64);
    dst[..n].copy_from_slice(&b[..n]);
}

pub fn sys_uname(buf: usize) -> SysResult {
    let mut u = Utsname::default();
    fill(&mut u.sysname, "Linux");
    fill(&mut u.nodename, "jiege");
    fill(&mut u.release, "6.6.0-jiege");
    fill(&mut u.version, "#1 SMP Fri Sep 5 2026");
    fill(&mut u.machine, "riscv64");
    fill(&mut u.domainname, "(none)");
    write_val(buf, u)?;
    Ok(0)
}

pub fn sys_sysinfo(buf: usize) -> SysResult {
    let (used, total) = crate::mm::heap::stats();
    let si = Sysinfo {
        uptime: (monotonic_ns() / 1_000_000_000) as i64,
        loads: [0; 3],
        totalram: total as u64,
        freeram: (total - used) as u64,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: crate::task::PROCESSES.lock().len() as u16,
        pad: 0,
        _pad2: 0,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        _f: [0; 4],
    };
    write_val(buf, si)?;
    Ok(0)
}

pub fn sys_getrandom(buf: usize, len: usize, _flags: u32) -> SysResult {
    let n = len.min(1024 * 1024);
    let mut v = alloc::vec![0u8; n];
    crate::fs::devices::fill_random(&mut v);
    copy_to_user(buf, &v)?;
    Ok(n)
}

pub fn sys_getcpu(cpu: usize, node: usize) -> SysResult {
    if cpu != 0 {
        write_val(cpu, 0u32)?;
    }
    if node != 0 {
        write_val(node, 0u32)?;
    }
    Ok(0)
}

pub fn sys_reboot(_magic1: u32, _magic2: u32, cmd: u32) -> SysResult {
    match cmd {
        0x4321fedc | 0xcdef0123 | 0x01234567 => {
            crate::println!("[kernel] reboot/poweroff requested");
            crate::sbi::shutdown();
        }
        _ => Ok(0),
    }
}
