use crate::mm::translated_byte_buffer;
use crate::task::{current_task, current_user_token};
use spin::Mutex;

pub fn sys_getpid() -> isize {
    crate::task::current_pid() as isize
}

pub fn sys_getppid() -> isize {
    let task = current_task().unwrap();
    let inner = task.inner_lock();
    match &inner.parent {
        Some(p) => p.upgrade().map(|p| p.pid() as isize).unwrap_or(1),
        None => 0,
    }
}

pub fn sys_gettid() -> isize {
    crate::task::current_pid() as isize
}

pub fn sys_getuid() -> isize {
    0
}

pub fn sys_prctl(_option: usize, _arg2: usize) -> isize {
    0
}

pub fn sys_setuid_like(_a: usize) -> isize {
    0
}

pub fn sys_setgroups(_size: usize, _list: usize) -> isize {
    0
}

pub fn sys_rt_sig_stub() -> isize {
    0
}

pub fn sys_setitimer() -> isize {
    0
}

pub fn sys_ioctl() -> isize {
    0
}

pub fn sys_fcntl(_fd: usize, _cmd: usize, _arg: usize) -> isize {
    0
}

fn write_bytes(token: usize, ptr: *mut u8, bytes: &[u8]) {
    let mut chunks = translated_byte_buffer(token, ptr, bytes.len());
    let mut copied = 0;
    for chunk in chunks.iter_mut() {
        let n = chunk.len();
        chunk.copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
    }
}

pub fn sys_uname(buf: *mut u8) -> isize {
    let token = current_user_token();
    const FIELD: usize = 65;
    let mut out = [0u8; FIELD * 6];
    let fields: [&[u8]; 6] = [
        b"Linux",
        b"ijiege",
        b"6.1.0-ijiege",
        b"#1 SMP",
        b"riscv64",
        b"",
    ];
    for (i, f) in fields.iter().enumerate() {
        out[i * FIELD..i * FIELD + f.len()].copy_from_slice(f);
    }
    write_bytes(token, buf, &out);
    0
}

const TIMER_FREQ_HZ: u64 = 10_000_000;

fn now_sec_nsec() -> (i64, i64) {
    let ticks = riscv::register::time::read64();
    let sec = ticks / TIMER_FREQ_HZ;
    let nsec = (ticks % TIMER_FREQ_HZ) * (1_000_000_000 / TIMER_FREQ_HZ);
    (sec as i64, nsec as i64)
}

pub fn sys_clock_gettime(_clock_id: usize, buf: *mut u8) -> isize {
    let token = current_user_token();
    let (sec, nsec) = now_sec_nsec();
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&sec.to_ne_bytes());
    out[8..16].copy_from_slice(&nsec.to_ne_bytes());
    write_bytes(token, buf, &out);
    0
}

pub fn sys_gettimeofday(buf: *mut u8) -> isize {
    let token = current_user_token();
    let (sec, nsec) = now_sec_nsec();
    let usec = nsec / 1000;
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&sec.to_ne_bytes());
    out[8..16].copy_from_slice(&usec.to_ne_bytes());
    write_bytes(token, buf, &out);
    0
}

pub fn sys_sched_getaffinity(_pid: usize, cpusetsize: usize, buf: *mut u8) -> isize {
    let token = current_user_token();
    let n = cpusetsize.min(8);
    let mut out = alloc::vec![0u8; n];
    if n > 0 {
        out[0] = 1;
    }
    write_bytes(token, buf, &out);
    n as isize
}

const RLIMIT_NOFILE: usize = 7;

pub fn sys_prlimit64(_pid: usize, resource: usize, _new: usize, old: usize) -> isize {
    if old != 0 {
        let token = current_user_token();
        let (cur, max): (u64, u64) = if resource == RLIMIT_NOFILE {
            (65536, 65536)
        } else {
            (u64::MAX, u64::MAX)
        };
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&cur.to_ne_bytes());
        out[8..16].copy_from_slice(&max.to_ne_bytes());
        write_bytes(token, old as *mut u8, &out);
    }
    0
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
static RNG: Mutex<Rng> = Mutex::new(Rng(0x2545F4914F6CDD1D));

pub fn sys_getrandom(buf: *mut u8, len: usize, _flags: usize) -> isize {
    let token = current_user_token();
    let mut out = alloc::vec![0u8; len];
    let mut rng = RNG.lock();
    for chunk in out.chunks_mut(8) {
        let r = rng.next().to_ne_bytes();
        chunk.copy_from_slice(&r[..chunk.len()]);
    }
    drop(rng);
    write_bytes(token, buf, &out);
    len as isize
}
