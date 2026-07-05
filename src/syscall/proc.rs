//! Process/identity/signal/misc syscalls.
use super::*;
use crate::task::current;

pub fn exit(code: i32) -> SysResult {
    println!("[proc] user exited with code {}", code);
    current().exit_code = Some(code);
    // Returning normally lets the kernel loop notice exit_code and stop.
    Ok(0)
}

pub fn set_tid_address(addr: usize) -> SysResult {
    current().tid_address = addr;
    Ok(1) // tid
}

pub fn futex(uaddr: usize, op: usize, val: usize, _timeout: usize) -> SysResult {
    const FUTEX_WAIT: usize = 0;
    const FUTEX_WAKE: usize = 1;
    match op & 0x7f {
        FUTEX_WAIT => {
            let cur: u32 = read_user(uaddr)?;
            if cur != val as u32 {
                return Err(EAGAIN);
            }
            // Single-threaded: waiting would deadlock. Treat as spurious wakeup.
            Ok(0)
        }
        FUTEX_WAKE => Ok(0),
        _ => Ok(0),
    }
}

pub fn rt_sigaction(signum: usize, act: usize, oldact: usize) -> SysResult {
    let t = current();
    if oldact != 0 {
        let old = t.sigactions.get(&signum).copied().unwrap_or([0; 4]);
        for (i, v) in old.iter().enumerate() {
            write_user(oldact + i * 8, *v)?;
        }
    }
    if act != 0 {
        let mut a = [0usize; 4];
        for (i, v) in a.iter_mut().enumerate() {
            *v = read_user(act + i * 8)?;
        }
        t.sigactions.insert(signum, a);
    }
    Ok(0)
}

pub fn rt_sigprocmask(how: usize, set: usize, oldset: usize) -> SysResult {
    let t = current();
    if oldset != 0 {
        write_user(oldset, t.sigmask)?;
    }
    if set != 0 {
        let s: u64 = read_user(set)?;
        t.sigmask = match how {
            0 => t.sigmask | s,  // BLOCK
            1 => t.sigmask & !s, // UNBLOCK
            2 => s,              // SETMASK
            _ => return Err(EINVAL),
        };
    }
    Ok(0)
}

pub fn sched_getaffinity(_pid: usize, size: usize, mask: usize) -> SysResult {
    if size < 8 {
        return Err(EINVAL);
    }
    write_user(mask, 1u64)?; // one CPU
    Ok(8)
}

pub fn uname(buf: usize) -> SysResult {
    // struct utsname: 6 fields x 65 bytes
    let fields: [&str; 6] = [
        "Linux",
        "jiege-os",
        "6.6.0-jiege",
        "#1 JiegeOS",
        "riscv64",
        "(none)",
    ];
    check_user_range(buf, 65 * 6)?;
    unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 65 * 6) };
    for (i, f) in fields.iter().enumerate() {
        let dst = user_slice_mut(buf + i * 65, f.len())?;
        dst.copy_from_slice(f.as_bytes());
    }
    Ok(0)
}

pub fn getrlimit(resource: usize, rlim: usize) -> SysResult {
    let val: u64 = match resource {
        7 => 1024,          // RLIMIT_NOFILE
        3 => 8 * 1024 * 1024, // RLIMIT_STACK
        _ => u64::MAX,
    };
    write_user(rlim, val)?;
    write_user(rlim + 8, val)?;
    Ok(0)
}

pub fn prlimit64(_pid: usize, resource: usize, new: usize, old: usize) -> SysResult {
    if old != 0 {
        getrlimit(resource, old)?;
    }
    let _ = new; // accept any new limit
    Ok(0)
}

pub fn getrusage(_who: usize, buf: usize) -> SysResult {
    check_user_range(buf, 144)?;
    unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 144) };
    Ok(0)
}

pub fn sysinfo(buf: usize) -> SysResult {
    check_user_range(buf, 112)?;
    unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 112) };
    write_user(buf, crate::time::uptime_seconds() as i64)?; // uptime
    write_user(buf + 32, 1024u64 * 1024 * 1024)?; // totalram
    write_user(buf + 40, 512u64 * 1024 * 1024)?; // freeram
    write_user(buf + 100, 1u32)?; // mem_unit... offset approximate
    Ok(0)
}

pub fn getrandom(buf: usize, len: usize, _flags: usize) -> SysResult {
    let dst = user_slice_mut(buf, len)?;
    // xorshift seeded from cycle counter — fine for nginx's needs
    let mut seed = crate::time::now_ns() as u64 ^ 0x9e3779b97f4a7c15;
    for b in dst.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *b = seed as u8;
    }
    Ok(len)
}

pub fn riscv_flush_icache() -> SysResult {
    unsafe { core::arch::asm!("fence.i") };
    Ok(0)
}
