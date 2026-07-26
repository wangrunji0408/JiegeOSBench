//! Time, identity, and other odds and ends.

use crate::fs::stat::{SysInfo, Timespec, Timeval, Tms, UtsName};
use crate::fs::Result;
use crate::mm::uaccess;
use crate::{bail, task};

const CLOCK_REALTIME: u32 = 0;
const CLOCK_MONOTONIC: u32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: u32 = 2;
const CLOCK_THREAD_CPUTIME_ID: u32 = 3;
const CLOCK_MONOTONIC_RAW: u32 = 4;
const CLOCK_REALTIME_COARSE: u32 = 5;
const CLOCK_MONOTONIC_COARSE: u32 = 6;
const CLOCK_BOOTTIME: u32 = 7;

fn clock_value(clock: u32) -> Result<(u64, u64)> {
    match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => Ok(crate::time::realtime()),
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_MONOTONIC_COARSE | CLOCK_BOOTTIME => {
            Ok(crate::time::monotonic())
        }
        CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {
            // Approximate CPU time from the ticks we charged the process.
            let task = task::current();
            let ticks = task.group.utime.load(core::sync::atomic::Ordering::Relaxed) as u64;
            let ns = ticks * (1_000_000_000 / crate::time::TICK_HZ);
            Ok((ns / 1_000_000_000, ns % 1_000_000_000))
        }
        _ => bail!(EINVAL),
    }
}

pub fn sys_clock_gettime(clock: u32, ptr: usize) -> Result<isize> {
    let (sec, nsec) = clock_value(clock)?;
    uaccess::write(
        ptr,
        Timespec {
            sec: sec as i64,
            nsec: nsec as i64,
        },
    )?;
    Ok(0)
}

pub fn sys_clock_settime(clock: u32, ptr: usize) -> Result<isize> {
    if clock != CLOCK_REALTIME {
        bail!(EINVAL);
    }
    let ts: Timespec = uaccess::read(ptr)?;
    crate::time::set_realtime(ts.sec as u64);
    Ok(0)
}

pub fn sys_clock_getres(clock: u32, ptr: usize) -> Result<isize> {
    // Validate the clock, then report our tick resolution (100 ns).
    clock_value(clock)?;
    if ptr != 0 {
        uaccess::write(ptr, Timespec { sec: 0, nsec: 100 })?;
    }
    Ok(0)
}

const TIMER_ABSTIME: u32 = 1;

pub fn sys_clock_nanosleep(
    clock: u32,
    flags: u32,
    request_ptr: usize,
    remain_ptr: usize,
) -> Result<isize> {
    let request: Timespec = uaccess::read(request_ptr)?;
    if request.sec < 0 || request.nsec < 0 || request.nsec >= 1_000_000_000 {
        bail!(EINVAL);
    }
    let target_ms = if flags & TIMER_ABSTIME != 0 {
        // Absolute: convert against the requested clock.
        let (now_sec, now_nsec) = clock_value(clock)?;
        let now_ms = now_sec * 1000 + now_nsec / 1_000_000;
        let want_ms = (request.sec as u64) * 1000 + (request.nsec as u64) / 1_000_000;
        crate::time::monotonic_ms() + want_ms.saturating_sub(now_ms)
    } else {
        let ms = (request.sec as u64) * 1000 + (request.nsec as u64 + 999_999) / 1_000_000;
        crate::time::monotonic_ms() + ms
    };

    while crate::time::monotonic_ms() < target_ms {
        if task::has_pending_signal() {
            if remain_ptr != 0 && flags & TIMER_ABSTIME == 0 {
                let left_ms = target_ms.saturating_sub(crate::time::monotonic_ms());
                uaccess::write(
                    remain_ptr,
                    Timespec {
                        sec: (left_ms / 1000) as i64,
                        nsec: ((left_ms % 1000) * 1_000_000) as i64,
                    },
                )?;
            }
            bail!(EINTR);
        }
        // Keep the network stack alive while we sleep; nginx's master process
        // spends most of its life here.
        crate::net::poll();
        task::yield_now();
    }
    if remain_ptr != 0 && flags & TIMER_ABSTIME == 0 {
        uaccess::write(remain_ptr, Timespec { sec: 0, nsec: 0 })?;
    }
    Ok(0)
}

pub fn sys_nanosleep(request_ptr: usize, remain_ptr: usize) -> Result<isize> {
    sys_clock_nanosleep(CLOCK_MONOTONIC, 0, request_ptr, remain_ptr)
}

pub fn sys_gettimeofday(tv_ptr: usize, tz_ptr: usize) -> Result<isize> {
    if tv_ptr != 0 {
        let (sec, nsec) = crate::time::realtime();
        uaccess::write(
            tv_ptr,
            Timeval {
                sec: sec as i64,
                usec: (nsec / 1000) as i64,
            },
        )?;
    }
    if tz_ptr != 0 {
        // `struct timezone { int tz_minuteswest; int tz_dsttime; }` — UTC.
        uaccess::write(tz_ptr, [0i32, 0i32])?;
    }
    Ok(0)
}

pub fn sys_times(ptr: usize) -> Result<isize> {
    let task = task::current();
    use core::sync::atomic::Ordering;
    let utime = task.group.utime.load(Ordering::Relaxed) as i64;
    let stime = task.group.stime.load(Ordering::Relaxed) as i64;
    let (cutime, cstime) = {
        let children = task.group.children.lock();
        (
            children
                .iter()
                .map(|c| c.group.utime.load(Ordering::Relaxed) as i64)
                .sum(),
            children
                .iter()
                .map(|c| c.group.stime.load(Ordering::Relaxed) as i64)
                .sum(),
        )
    };
    if ptr != 0 {
        uaccess::write(
            ptr,
            Tms {
                utime,
                stime,
                cutime,
                cstime,
            },
        )?;
    }
    // `times` returns the elapsed time in clock ticks.
    Ok(crate::time::ticks() as isize)
}

/// `struct itimerval`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ITimerVal {
    interval: Timeval,
    value: Timeval,
}

/// Interval timers. We track the request and deliver SIGALRM for `ITIMER_REAL`,
/// which is what `alarm()` compiles to.
static ITIMER: spin::Mutex<[(u64, u64, usize); 3]> = spin::Mutex::new([(0, 0, 0); 3]);

pub fn sys_setitimer(which: u32, new_ptr: usize, old_ptr: usize) -> Result<isize> {
    if which > 2 {
        bail!(EINVAL);
    }
    let mut timers = ITIMER.lock();
    let (deadline_ms, interval_ms, owner) = timers[which as usize];

    if old_ptr != 0 {
        let now = crate::time::monotonic_ms();
        let remaining = deadline_ms.saturating_sub(now);
        let old = ITimerVal {
            interval: Timeval {
                sec: (interval_ms / 1000) as i64,
                usec: ((interval_ms % 1000) * 1000) as i64,
            },
            value: Timeval {
                sec: (remaining / 1000) as i64,
                usec: ((remaining % 1000) * 1000) as i64,
            },
        };
        drop(timers);
        uaccess::write(old_ptr, old)?;
        timers = ITIMER.lock();
    }

    if new_ptr != 0 {
        let new: ITimerVal = uaccess::read(new_ptr)?;
        let value_ms = (new.value.sec as u64) * 1000 + (new.value.usec as u64) / 1000;
        let interval_ms = (new.interval.sec as u64) * 1000 + (new.interval.usec as u64) / 1000;
        timers[which as usize] = if value_ms == 0 {
            (0, 0, 0)
        } else {
            (
                crate::time::monotonic_ms() + value_ms,
                interval_ms,
                task::current().pid(),
            )
        };
    }
    let _ = owner;
    Ok(0)
}

pub fn sys_getitimer(which: u32, ptr: usize) -> Result<isize> {
    if which > 2 {
        bail!(EINVAL);
    }
    let (deadline_ms, interval_ms, _) = ITIMER.lock()[which as usize];
    let remaining = deadline_ms.saturating_sub(crate::time::monotonic_ms());
    uaccess::write(
        ptr,
        ITimerVal {
            interval: Timeval {
                sec: (interval_ms / 1000) as i64,
                usec: ((interval_ms % 1000) * 1000) as i64,
            },
            value: Timeval {
                sec: (remaining / 1000) as i64,
                usec: ((remaining % 1000) * 1000) as i64,
            },
        },
    )?;
    Ok(0)
}

/// Called from the timer tick: fire any expired interval timers.
pub fn check_itimers() {
    let now = crate::time::monotonic_ms();
    let mut fire: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();
    {
        let mut timers = ITIMER.lock();
        for (which, timer) in timers.iter_mut().enumerate() {
            if timer.0 != 0 && now >= timer.0 {
                fire.push((which, timer.2));
                if timer.1 > 0 {
                    timer.0 = now + timer.1;
                } else {
                    *timer = (0, 0, 0);
                }
            }
        }
    }
    for (which, pid) in fire {
        let sig = match which {
            0 => crate::signal::SIGALRM,
            1 => 27, // SIGVTALRM
            _ => 26, // SIGPROF
        };
        if let Some(target) = task::find_process(pid) {
            crate::signal::send_to_process(&target, sig);
        }
    }
}

pub fn sys_uname(ptr: usize) -> Result<isize> {
    uaccess::write(ptr, UtsName::new())?;
    Ok(0)
}

pub fn sys_sysinfo(ptr: usize) -> Result<isize> {
    let (used, total) = crate::mm::frame::stats();
    let (uptime, _) = crate::time::monotonic();
    let info = SysInfo {
        uptime: uptime as i64,
        loads: [0; 3],
        totalram: (total * crate::mm::PAGE_SIZE) as u64,
        freeram: ((total - used) * crate::mm::PAGE_SIZE) as u64,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: task::all_tasks().len() as u16,
        pad: 0,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        _f: [],
    };
    uaccess::write(ptr, info)?;
    Ok(0)
}

pub fn sys_getrandom(buf: usize, len: usize, _flags: u32) -> Result<isize> {
    if len == 0 {
        return Ok(0);
    }
    let n = len.min(1 << 20);
    let mut data = alloc::vec![0u8; n];
    crate::fs::device::fill_random(&mut data);
    uaccess::write_bytes(buf, &data)?;
    Ok(n as isize)
}

/// `riscv_hwprobe` key/value pair.
#[repr(C)]
#[derive(Clone, Copy)]
struct HwProbePair {
    key: i64,
    value: u64,
}

const RISCV_HWPROBE_KEY_MVENDORID: i64 = 0;
const RISCV_HWPROBE_KEY_MARCHID: i64 = 1;
const RISCV_HWPROBE_KEY_MIMPID: i64 = 2;
const RISCV_HWPROBE_KEY_BASE_BEHAVIOR: i64 = 3;
const RISCV_HWPROBE_KEY_IMA_EXT_0: i64 = 4;
const RISCV_HWPROBE_KEY_CPUPERF_0: i64 = 5;

pub fn sys_riscv_hwprobe(
    pairs_ptr: usize,
    pair_count: usize,
    _cpu_count: usize,
    _cpus_ptr: usize,
    _flags: u32,
) -> Result<isize> {
    // musl's ifunc resolvers use this to pick optimized string routines.
    for i in 0..pair_count.min(64) {
        let offset = pairs_ptr + i * core::mem::size_of::<HwProbePair>();
        let mut pair: HwProbePair = uaccess::read(offset)?;
        pair.value = match pair.key {
            RISCV_HWPROBE_KEY_MVENDORID | RISCV_HWPROBE_KEY_MARCHID | RISCV_HWPROBE_KEY_MIMPID => 0,
            // RISCV_HWPROBE_BASE_BEHAVIOR_IMA
            RISCV_HWPROBE_KEY_BASE_BEHAVIOR => 1,
            // FD | C  (we support the full rv64gc set QEMU gives us)
            RISCV_HWPROBE_KEY_IMA_EXT_0 => 0x1 | 0x2,
            // MISALIGNED_UNKNOWN
            RISCV_HWPROBE_KEY_CPUPERF_0 => 0,
            _ => {
                // An unknown key must be reported by setting it to -1.
                pair.key = -1;
                0
            }
        };
        uaccess::write(offset, pair)?;
    }
    Ok(0)
}
