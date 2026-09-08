//! Linux riscv64 (asm-generic) system call dispatch.

use crate::errno::*;
use crate::fs::file::*;
use crate::fs::{self, Inode, Kind};
use crate::socket::{self, Socket};
use crate::task::process::{Process, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::task::signal::{self, SigAction, SigInfo, SignalState};
use crate::trap::TrapFrame;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const AT_FDCWD: i32 = -100;
pub const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
pub const AT_REMOVEDIR: u32 = 0x200;
pub const AT_SYMLINK_FOLLOW: u32 = 0x400;
pub const AT_EMPTY_PATH: u32 = 0x1000;

pub const WNOHANG: u32 = 1;
pub const WUNTRACED: u32 = 2;

const RLIMIT_NOFILE: usize = 7;

pub fn handle(tf: &mut TrapFrame) {
    let nr = tf.a7();
    let t = crate::task::current();
    let p = match t.process.as_mut() {
        Some(p) => p,
        None => {
            tf.set_a0((-ENOSYS as isize) as usize);
            return;
        }
    };
    let args = [
        tf.a0(),
        tf.a1(),
        tf.a2(),
        tf.a3(),
        tf.x[14],
        tf.x[15],
    ];
    match dispatch(nr, &args, tf, p) {
        Ok(v) => tf.set_a0(v as usize),
        Err(e) => tf.set_a0((-e as isize) as usize),
    }
}

fn dispatch(nr: usize, a: &[usize; 6], tf: &mut TrapFrame, p: &mut Process) -> Result<usize, Errno> {
    match nr {
        // ---- process / identity ----
        172 => Ok(p.pid),                                  // getpid
        173 => Ok(p.ppid),                                 // getppid
        178 => Ok(p.pid),                                  // gettid
        174 | 175 => Ok(p.euid as usize),                  // getuid / geteuid
        176 | 177 => Ok(p.egid as usize),                  // getgid / getegid
        148 => {
            // getresuid
            let (r, e, s) = (p.uid as u32, p.euid as u32, p.uid as u32);
            write_u32(p, a[0], r)?;
            write_u32(p, a[1], e)?;
            write_u32(p, a[2], s)?;
            Ok(0)
        }
        150 => {
            let (r, e, s) = (p.gid as u32, p.egid as u32, p.gid as u32);
            write_u32(p, a[0], r)?;
            write_u32(p, a[1], e)?;
            write_u32(p, a[2], s)?;
            Ok(0)
        }
        146 => {
            // setuid
            p.uid = a[0] as u32;
            p.euid = a[0] as u32;
            Ok(0)
        }
        144 => {
            p.gid = a[0] as u32;
            p.egid = a[0] as u32;
            Ok(0)
        }
        147 => {
            p.uid = a[0] as u32;
            p.euid = a[1] as u32;
            Ok(0)
        }
        149 => {
            p.gid = a[0] as u32;
            p.egid = a[1] as u32;
            Ok(0)
        }
        159 => Ok(0), // setgroups
        158 => {
            // getgroups
            write_u32(p, a[1], p.gid)?;
            Ok(1)
        }
        154 => {
            // setpgid
            p.pgid = if a[1] == 0 { p.pid } else { a[1] };
            Ok(0)
        }
        155 => Ok(p.pgid), // getpgid
        157 => {
            p.sid = p.pid;
            p.pgid = p.pid;
            Ok(p.pid)
        }
        156 => Ok(p.sid), // getsid
        167 => Ok(0),     // prctl
        166 => {
            let old = p.umask;
            p.umask = a[0] as u32;
            Ok(old as usize)
        }
        160 => {
            // uname
            let mut buf = [0u8; 390];
            let fields = [
                ("Linux", 0usize),
                ("localhost", 65),
                ("6.8.0-ijiege", 130),
                ("#1 SMP riscv64", 195),
                ("riscv64", 260),
                ("(none)", 325),
            ];
            for (s, off) in fields.iter() {
                let b = s.as_bytes();
                let n = b.len().min(64);
                buf[*off..*off + n].copy_from_slice(&b[..n]);
            }
            p.copy_to_user(a[0], &buf)?;
            Ok(0)
        }
        96 => {
            // set_tid_address
            let old = p.clear_child_tid;
            p.clear_child_tid = a[0];
            Ok(if old != 0 { old } else { p.pid })
        }
        99 => {
            p.robust_list = a[0];
            p.robust_list_len = a[1];
            Ok(0)
        }
        100 => {
            write_u64(p, a[0], p.robust_list as u64)?;
            Ok(0)
        }
        261 => {
            // prlimit64
            let res = a[1];
            let (cur, max): (u64, u64) = match res {
                RLIMIT_NOFILE => (4096, 4096),
                3 => (8 * 1024 * 1024, u64::MAX), // RLIMIT_STACK
                4 => (0, u64::MAX),               // RLIMIT_CORE
                6 => (1024, 1024),                // RLIMIT_NPROC
                _ => (u64::MAX, u64::MAX),
            };
            if a[3] != 0 {
                let mut buf = [0u8; 16];
                buf[0..8].copy_from_slice(&cur.to_le_bytes());
                buf[8..16].copy_from_slice(&max.to_le_bytes());
                p.copy_to_user(a[3], &buf)?;
            }
            if a[2] != 0 {
                // accept new limits silently
            }
            Ok(0)
        }
        163 => {
            // getrlimit
            let (cur, max): (u64, u64) = match a[0] {
                RLIMIT_NOFILE => (4096, 4096),
                3 => (8 * 1024 * 1024, u64::MAX),
                4 => (0, u64::MAX),
                _ => (u64::MAX, u64::MAX),
            };
            let mut buf = [0u8; 16];
            buf[0..8].copy_from_slice(&cur.to_le_bytes());
            buf[8..16].copy_from_slice(&max.to_le_bytes());
            p.copy_to_user(a[1], &buf)?;
            Ok(0)
        }
        164 => Ok(0), // setrlimit
        168 => {
            write_u32(p, a[0], 0)?;
            write_u32(p, a[1], 0)?;
            Ok(0)
        }
        179 => {
            // sysinfo
            let mut buf = [0u8; 112];
            let total = (crate::mm::frame::total_count() * 4096) as u64;
            let free = (crate::mm::frame::free_count() * 4096) as u64;
            buf[32..40].copy_from_slice(&total.to_le_bytes()); // totalram
            buf[40..48].copy_from_slice(&free.to_le_bytes()); // freeram
            buf[104..112].copy_from_slice(&1u64.to_le_bytes()); // procs
            p.copy_to_user(a[0], &buf)?;
            Ok(0)
        }
        121 => Ok(0),  // sched_setscheduler
        120 => Ok(0),  // sched_getscheduler
        124 => {
            crate::task::schedule();
            Ok(0)
        }
        122 => Ok(0), // sched_setaffinity
        123 => {
            // sched_getaffinity
            let len = a[1];
            if len >= 8 {
                write_u64(p, a[2], 1)?;
                if len > 8 {
                    let z = alloc::vec![0u8; len - 8];
                    p.copy_to_user(a[2] + 8, &z)?;
                }
            }
            Ok(8)
        }
        128 => Err(EINTR), // restart_syscall
        92 => Ok(0),       // personality
        283 => Ok(0),      // membarrier
        293 => Err(ENOSYS), // rseq
        258 => Err(ENOSYS), // riscv_hwprobe
        291 => Err(ENOSYS), // statx
        278 => {
            // getrandom
            let mut buf = alloc::vec![0u8; a[1].min(4096)];
            crate::fs::chardev::read(crate::fs::chardev::DEV_URANDOM, &mut buf)?;
            p.copy_to_user(a[0], &buf)?;
            Ok(buf.len())
        }
        116 => Ok(0), // syslog

        // ---- memory ----
        214 => {
            // brk
            let cur = p.mm.brk;
            if a[0] != 0 {
                let new = p.brk(a[0]);
                if new != a[0] {
                    return Ok(cur);
                }
            }
            Ok(p.mm.brk)
        }
        222 => {
            // mmap
            let prot = a[2] as u32;
            let flags = a[3] as u32;
            let fd = a[4] as i32;
            let off = a[5] as u64;
            let file = if flags & crate::task::process::MAP_ANONYMOUS == 0 && fd >= 0 {
                let f = p.files.get(fd)?;
                let inode = f.inode_of().ok_or(ENODEV)?;
                Some((inode, off))
            } else {
                None
            };
            p.mmap(a[0], a[1], prot, flags, file)
        }
        215 => {
            p.munmap(a[0], a[1])?;
            Ok(0)
        }
        226 => {
            p.mprotect(a[0], a[1], a[2] as u32)?;
            Ok(0)
        }
        216 => {
            // mremap
            let old_addr = a[0];
            let old_size = a[1];
            let new_size = a[2];
            let flags = a[3] as u32;
            if flags & 2 != 0 {
                // MREMAP_FIXED
                let new_addr = a[4];
                let mut tmp = alloc::vec![0u8; old_size.min(new_size)];
                p.copy_from_user(old_addr, &mut tmp)?;
                p.munmap(old_addr, old_size)?;
                let r = p.mmap(
                    new_addr,
                    new_size,
                    PROT_READ | PROT_WRITE,
                    crate::task::process::MAP_FIXED
                        | crate::task::process::MAP_ANONYMOUS
                        | crate::task::process::MAP_PRIVATE,
                    None,
                )?;
                p.copy_to_user(r, &tmp)?;
                Ok(r)
            } else if new_size <= old_size {
                Ok(old_addr)
            } else {
                let extra = new_size - old_size;
                let r = p.mmap(
                    old_addr + old_size,
                    extra,
                    PROT_READ | PROT_WRITE,
                    crate::task::process::MAP_FIXED
                        | crate::task::process::MAP_ANONYMOUS
                        | crate::task::process::MAP_PRIVATE,
                    None,
                )?;
                if r != old_addr + old_size {
                    return Err(ENOMEM);
                }
                Ok(old_addr)
            }
        }
        227 | 233 | 228 | 229 | 230 | 231 | 232 => Ok(0), // msync/madvise/mlock*

        // ---- files ----
        56 => sys_openat(p, a[0] as i32, a[1], a[2] as u32, a[3] as u32),
        57 => {
            p.files.close(a[0] as i32)?;
            Ok(0)
        }
        63 => {
            // read
            let f = p.files.get(a[0] as i32)?;
            let mut buf = alloc::vec![0u8; a[2].min(1 << 20)];
            let mut n = 0;
            loop {
                match f.read(&mut buf) {
                    Ok(k) => {
                        n = k;
                        break;
                    }
                    Err(EAGAIN) => {
                        if f.nonblocking() {
                            return Err(EAGAIN);
                        }
                        if let Some(s) = signal_pending(p) {
                            return Err(if s { EINTR } else { EAGAIN });
                        }
                        crate::task::sleep_ticks(1);
                    }
                    Err(e) => return Err(e),
                }
            }
            p.copy_to_user(a[1], &buf[..n])?;
            Ok(n)
        }
        64 => {
            let f = p.files.get(a[0] as i32)?;
            let mut buf = alloc::vec![0u8; a[2].min(1 << 20)];
            p.copy_from_user(a[1], &mut buf)?;
            write_all(p, &f, &buf)
        }
        65 => {
            // readv
            let f = p.files.get(a[0] as i32)?;
            let iovs = read_iovs(p, a[1], a[2])?;
            let mut total = 0;
            for (base, len) in iovs {
                let mut buf = alloc::vec![0u8; len.min(1 << 20)];
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        p.copy_to_user(base, &buf[..n])?;
                        total += n;
                        if n < len {
                            break;
                        }
                    }
                    Err(EAGAIN) => {
                        if total > 0 {
                            break;
                        }
                        if f.nonblocking() {
                            return Err(EAGAIN);
                        }
                        crate::task::sleep_ticks(1);
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(total)
        }
        66 => {
            // writev
            let f = p.files.get(a[0] as i32)?;
            let iovs = read_iovs(p, a[1], a[2])?;
            let mut total = 0;
            for (base, len) in iovs {
                let mut buf = alloc::vec![0u8; len.min(1 << 20)];
                p.copy_from_user(base, &mut buf)?;
                match write_all(p, &f, &buf) {
                    Ok(n) => total += n,
                    Err(EAGAIN) if total > 0 => break,
                    Err(e) => return Err(e),
                }
            }
            Ok(total)
        }
        67 => {
            // pread64
            let f = p.files.get(a[0] as i32)?;
            let mut buf = alloc::vec![0u8; a[2].min(1 << 20)];
            let n = match &mut *f.obj.lock() {
                FileObj::Regular { inode, .. } => inode.read_at(a[3] as u64, &mut buf)?,
                _ => {
                    let off = f.lseek(0, SEEK_CUR)?;
                    let n = f.read(&mut buf)?;
                    f.lseek(off as i64, SEEK_SET)?;
                    n
                }
            };
            p.copy_to_user(a[1], &buf[..n])?;
            Ok(n)
        }
        68 => {
            // pwrite64
            let f = p.files.get(a[0] as i32)?;
            let mut buf = alloc::vec![0u8; a[2].min(1 << 20)];
            p.copy_from_user(a[1], &mut buf)?;
            let n = match &mut *f.obj.lock() {
                FileObj::Regular { inode, .. } => inode.write_at(a[3] as u64, &buf)?,
                _ => return Err(ESPIPE),
            };
            Ok(n)
        }
        62 => {
            let f = p.files.get(a[0] as i32)?;
            f.lseek(a[1] as i64, a[2] as u32).map(|v| v as usize)
        }
        80 => {
            // fstat
            let f = p.files.get(a[0] as i32)?;
            let st = f.stat()?;
            write_stat(p, a[1], &st)?;
            Ok(0)
        }
        79 => {
            // newfstatat
            let dirfd = a[0] as i32;
            let path = p.read_cstr(a[1], 4096)?;
            let flags = a[3] as u32;
            if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
                let f = p.files.get(dirfd)?;
                let st = f.stat()?;
                write_stat(p, a[2], &st)?;
                return Ok(0);
            }
            let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
            let base = resolve_dir(p, dirfd, &path)?;
            let node = fs::lookup(&base, &path, follow)?;
            write_stat(p, a[2], &node.stat())?;
            Ok(0)
        }
        61 => sys_getdents64(p, a[0] as i32, a[1], a[2]),
        34 => {
            let path = p.read_cstr(a[1], 4096)?;
            let base = resolve_dir(p, a[0] as i32, &path)?;
            fs::create_at(&base, &path, a[2] as u32, Kind::Dir)?;
            Ok(0)
        }
        35 => {
            let path = p.read_cstr(a[1], 4096)?;
            let base = resolve_dir(p, a[0] as i32, &path)?;
            fs::unlink_at(&base, &path)?;
            Ok(0)
        }
        38 => {
            let old = p.read_cstr(a[1], 4096)?;
            let new = p.read_cstr(a[3], 4096)?;
            let b1 = resolve_dir(p, a[0] as i32, &old)?;
            let b2 = resolve_dir(p, a[2] as i32, &new)?;
            let node = fs::lookup(&b1, &old, false)?;
            let (np, nn) = fs::lookup_parent(&b2, &new)?;
            np.add_child(&nn, node);
            Ok(0)
        }
        276 => {
            let old = p.read_cstr(a[1], 4096)?;
            let new = p.read_cstr(a[3], 4096)?;
            let b1 = resolve_dir(p, a[0] as i32, &old)?;
            let b2 = resolve_dir(p, a[2] as i32, &new)?;
            let node = fs::lookup(&b1, &old, false)?;
            let (np, nn) = fs::lookup_parent(&b2, &new)?;
            np.add_child(&nn, node);
            Ok(0)
        }
        36 => {
            let target = p.read_cstr(a[0], 4096)?;
            let link = p.read_cstr(a[2], 4096)?;
            let base = resolve_dir(p, a[1] as i32, &link)?;
            fs::symlink_at(&base, &target, &link)?;
            Ok(0)
        }
        37 => {
            let old = p.read_cstr(a[1], 4096)?;
            let new = p.read_cstr(a[3], 4096)?;
            let b1 = resolve_dir(p, a[0] as i32, &old)?;
            let b2 = resolve_dir(p, a[2] as i32, &new)?;
            fs::link_at(&b1, &old, &new)?;
            let _ = b2;
            Ok(0)
        }
        78 => {
            // readlinkat
            let path = p.read_cstr(a[1], 4096)?;
            let base = resolve_dir(p, a[0] as i32, &path)?;
            let node = fs::lookup(&base, &path, false)?;
            let target = node.inner.lock().link.clone();
            let n = target.len().min(a[3]);
            p.copy_to_user(a[2], target.as_bytes()[..n].as_ref())?;
            Ok(n)
        }
        48 => {
            // faccessat
            let path = p.read_cstr(a[1], 4096)?;
            let base = resolve_dir(p, a[0] as i32, &path)?;
            match fs::lookup(&base, &path, true) {
                Ok(_) => Ok(0),
                Err(e) => Err(e),
            }
        }
        49 => {
            let path = p.read_cstr(a[0], 4096)?;
            let node = fs::lookup(&p.cwd.clone(), &path, true)?;
            if !node.is_dir() {
                return Err(ENOTDIR);
            }
            p.cwd = node;
            Ok(0)
        }
        50 => {
            let f = p.files.get(a[0] as i32)?;
            let node = f.inode_of().ok_or(ENOTDIR)?;
            if !node.is_dir() {
                return Err(ENOTDIR);
            }
            p.cwd = node;
            Ok(0)
        }
        17 => {
            // getcwd
            let path = b"/\0";
            if a[1] < path.len() {
                return Err(ERANGE);
            }
            p.copy_to_user(a[0], path)?;
            Ok(a[0])
        }
        23 => {
            // dup
            let f = p.files.get(a[0] as i32)?;
            let fd = p.files.alloc(f, a[1] as i32)?;
            Ok(fd as usize)
        }
        24 => {
            // dup3
            let f = p.files.get(a[0] as i32)?;
            if a[1] as i32 == a[0] as i32 {
                return Err(EINVAL);
            }
            p.files.set(a[1] as i32, f);
            Ok(a[1])
        }
        59 => {
            // pipe2
            let pipe = crate::fs::pipe::Pipe::new();
            let r = File::new(
                FileObj::Pipe {
                    pipe: pipe.clone(),
                    write_end: false,
                },
                a[1] as u32 & O_NONBLOCK,
                a[1] as u32 & O_CLOEXEC != 0,
            );
            let w = File::new(
                FileObj::Pipe {
                    pipe,
                    write_end: true,
                },
                a[1] as u32 & O_NONBLOCK,
                a[1] as u32 & O_CLOEXEC != 0,
            );
            let fd0 = p.files.alloc(r, 0)?;
            let fd1 = p.files.alloc(w, 0)?;
            write_u32(p, a[0], fd0 as u32)?;
            write_u32(p, a[0] + 4, fd1 as u32)?;
            Ok(0)
        }
        25 => {
            // fcntl
            let fd = a[0] as i32;
            let cmd = a[1] as i32;
            match cmd {
                0 => {
                    let f = p.files.get(fd)?;
                    Ok(p.files.alloc(f, a[2] as i32)? as usize)
                }
                1030 => {
                    let f = p.files.get(fd)?;
                    f.cloexec
                        .store(true, core::sync::atomic::Ordering::Relaxed);
                    Ok(p.files.alloc(f, a[2] as i32)? as usize)
                }
                1 => {
                    let f = p.files.get(fd)?;
                    Ok(f.cloexec.load(core::sync::atomic::Ordering::Relaxed) as usize)
                }
                2 => {
                    let f = p.files.get(fd)?;
                    f.cloexec
                        .store(a[2] & 1 != 0, core::sync::atomic::Ordering::Relaxed);
                    Ok(0)
                }
                3 => {
                    let f = p.files.get(fd)?;
                    Ok(*f.status.lock() as usize)
                }
                4 => {
                    let f = p.files.get(fd)?;
                    let mut s = f.status.lock();
                    *s = (*s & !(O_NONBLOCK | O_APPEND)) | (a[2] as u32 & (O_NONBLOCK | O_APPEND));
                    Ok(0)
                }
                8 | 9 | 6 | 5 => Ok(0), // F_SETOWN / F_GETOWN / locks
                _ => Ok(0),
            }
        }
        29 => {
            // ioctl
            let f = p.files.get(a[0] as i32)?;
            let req = a[1] as u32;
            match req {
                0x5421 => {
                    // FIONBIO
                    let mut v = [0u8; 4];
                    p.copy_from_user(a[2], &mut v)?;
                    let on = i32::from_le_bytes(v) != 0;
                    let mut s = f.status.lock();
                    if on {
                        *s |= O_NONBLOCK;
                    } else {
                        *s &= !O_NONBLOCK;
                    }
                    Ok(0)
                }
                0x5452 => Ok(0), // FIOASYNC
                0x541B => {
                    write_u32(p, a[2], 0)?;
                    Ok(0)
                }
                0x5401 => {
                    // TCGETS
                    let mut buf = [0u8; 60];
                    buf[0..4].copy_from_slice(&0x8a0u32.to_le_bytes()); // B115200|CS8|CREAD|CLOCAL
                    p.copy_to_user(a[2], &buf)?;
                    Ok(0)
                }
                0x5402 | 0x5403 | 0x5404 => Ok(0), // TCSETS*
                0x5413 => {
                    let mut buf = [0u8; 8];
                    buf[0..2].copy_from_slice(&24u16.to_le_bytes());
                    buf[2..4].copy_from_slice(&80u16.to_le_bytes());
                    p.copy_to_user(a[2], &buf)?;
                    Ok(0)
                }
                0x5410 => Ok(0), // TCFLSH
                0x5422 => Ok(0), // FIOASYNC alt
                _ => {
                    crate::println!("[ioctl] unhandled req={:#x} fd={}", req, a[0]);
                    Ok(0)
                }
            }
        }
        46 => {
            // ftruncate
            let f = p.files.get(a[0] as i32)?;
            let inode = f.inode_of().ok_or(EINVAL)?;
            inode.truncate(a[1] as u64)?;
            Ok(0)
        }
        45 => {
            let path = p.read_cstr(a[0], 4096)?;
            let node = fs::lookup(&p.cwd.clone(), &path, true)?;
            node.truncate(a[1] as u64)?;
            Ok(0)
        }
        54 | 55 => Ok(0), // fchownat / fchown
        52 | 53 => Ok(0), // fchmod / fchmodat
        88 => Ok(0),      // utimensat
        81 | 82 | 83 | 267 => Ok(0), // sync/fsync/fdatasync/syncfs
        43 | 44 => {
            // statfs / fstatfs
            let mut buf = [0u8; 120];
            buf[0..8].copy_from_slice(&0x01021994u64.to_le_bytes()); // TMPFS_MAGIC
            buf[8..16].copy_from_slice(&4096u64.to_le_bytes());
            buf[16..24].copy_from_slice(&(1u64 << 40).to_le_bytes());
            buf[32..40].copy_from_slice(&1_000_000u64.to_le_bytes());
            buf[40..48].copy_from_slice(&500_000u64.to_le_bytes());
            buf[48..56].copy_from_slice(&500_000u64.to_le_bytes());
            let dst = if nr == 43 { a[1] } else { a[1] };
            p.copy_to_user(dst, &buf)?;
            Ok(0)
        }
        71 => {
            // sendfile
            let out = p.files.get(a[0] as i32)?;
            let inp = p.files.get(a[1] as i32)?;
            let mut off = if a[2] != 0 {
                let mut b = [0u8; 8];
                p.copy_from_user(a[2], &mut b)?;
                u64::from_le_bytes(b)
            } else {
                inp.lseek(0, SEEK_CUR)?
            };
            let count = a[3];
            let mut buf = alloc::vec![0u8; count.min(1 << 20)];
            let n = match &mut *inp.obj.lock() {
                FileObj::Regular { inode, .. } => inode.read_at(off, &mut buf)?,
                _ => return Err(EINVAL),
            };
            if n == 0 {
                return Ok(0);
            }
            let w = write_all(p, &out, &buf[..n])?;
            off += w as u64;
            if a[2] != 0 {
                p.copy_to_user(a[2], &off.to_le_bytes())?;
            }
            Ok(w)
        }
        89 | 90 | 91 => Ok(0), // acct/capget/capset
        40 | 39 | 51 | 41 => Err(EPERM), // mount/umount/chroot/pivot_root

        // ---- signals ----
        134 => sys_rt_sigaction(p, a),
        135 => sys_rt_sigprocmask(p, a),
        136 => {
            write_u64(p, a[0], p.sig.pending & p.sig.blocked)?;
            Ok(0)
        }
        133 => {
            // rt_sigsuspend
            let mask = read_u64(p, a[0])?;
            let saved = p.sig.blocked;
            p.sig.blocked = mask;
            loop {
                if p.sig.pending & !p.sig.blocked != 0 {
                    p.sig.blocked = saved;
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
        }
        139 => signal::sigreturn(p, tf),
        132 => {
            // sigaltstack
            if a[1] != 0 {
                let mut b = [0u8; 24];
                b[0..8].copy_from_slice(&(p.sig.altstack_sp as u64).to_le_bytes());
                b[8..12].copy_from_slice(&p.sig.altstack_flags.to_le_bytes());
                b[16..24].copy_from_slice(&(p.sig.altstack_size as u64).to_le_bytes());
                p.copy_to_user(a[1], &b)?;
            }
            if a[0] != 0 {
                let mut b = [0u8; 24];
                p.copy_from_user(a[0], &mut b)?;
                p.sig.altstack_sp = u64::from_le_bytes(b[0..8].try_into().unwrap()) as usize;
                p.sig.altstack_flags = u32::from_le_bytes(b[8..12].try_into().unwrap());
                p.sig.altstack_size = u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize;
            }
            Ok(0)
        }
        129 | 130 | 131 => {
            // kill / tkill / tgkill
            let (pid, sig) = if nr == 129 {
                (a[0] as i32, a[1])
            } else if nr == 130 {
                (a[0] as i32, a[1])
            } else {
                (a[1] as i32, a[2])
            };
            if sig == 0 {
                return Ok(0);
            }
            if sig > 64 {
                return Err(EINVAL);
            }
            crate::task::send_signal(pid, sig, 0);
            Ok(0)
        }
        137 => {
            // rt_sigtimedwait: no support, report no signal
            Err(EAGAIN)
        }
        138 => Ok(0), // rt_sigqueueinfo

        // ---- scheduling / time ----
        93 => {
            // exit
            let code = a[0] as i32;
            crate::task::exit_current(code);
        }
        94 => {
            let code = a[0] as i32;
            crate::task::exit_current(code);
        }
        220 => {
            // clone(flags, stack, parent_tid, child_tid, tls)
            let flags = a[0];
            let stack = a[1];
            let ptid = a[2];
            let ctid = a[3];
            let tls = a[4];
            crate::task::sys_clone(p, flags, stack, ptid, ctid, tls, tf)
        }
        221 => {
            // execve
            let path = p.read_cstr(a[0], 4096)?;
            let argv = read_str_array(p, a[1])?;
            let envp = read_str_array(p, a[2])?;
            p.exec(&path, argv, envp)?;
            tf.sepc = p.init_tf.sepc;
            tf.x = p.init_tf.x;
            tf.sstatus = p.init_tf.sstatus;
            Ok(0)
        }
        260 => sys_wait4(p, a[0] as i32, a[1], a[2] as u32, a[3]),
        101 | 115 => {
            // nanosleep / clock_nanosleep
            let req = if nr == 101 { a[0] } else { a[2] };
            let mut b = [0u8; 16];
            p.copy_from_user(req, &mut b)?;
            let secs = u64::from_le_bytes(b[0..8].try_into().unwrap());
            let nsec = u64::from_le_bytes(b[8..16].try_into().unwrap());
            let ms = secs * 1000 + nsec / 1_000_000;
            let ticks = (ms / 4).max(1);
            crate::task::sleep_ticks(ticks);
            if let Some(rem) = if nr == 101 { a[1] } else { a[3] } {
                if rem != 0 {
                    let z = [0u8; 16];
                    p.copy_to_user(rem, &z)?;
                }
            }
            Ok(0)
        }
        169 => {
            // gettimeofday
            let ns = crate::time::realtime_ns();
            let mut tv = [0u8; 16];
            tv[0..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
            tv[8..16].copy_from_slice(&(ns % 1_000_000_000 / 1000).to_le_bytes());
            if a[0] != 0 {
                p.copy_to_user(a[0], &tv)?;
            }
            if a[1] != 0 {
                p.copy_to_user(a[1], &[0u8; 8])?;
            }
            Ok(0)
        }
        113 => {
            // clock_gettime
            let (sec, nsec) = clock_now(a[0]);
            let mut ts = [0u8; 16];
            ts[0..8].copy_from_slice(&sec.to_le_bytes());
            ts[8..16].copy_from_slice(&nsec.to_le_bytes());
            p.copy_to_user(a[1], &ts)?;
            Ok(0)
        }
        114 => {
            // clock_getres
            let mut ts = [0u8; 16];
            ts[8..16].copy_from_slice(&1_000_000u64.to_le_bytes());
            p.copy_to_user(a[1], &ts)?;
            Ok(0)
        }
        112 => Ok(0), // clock_settime
        153 => {
            // times
            let t = crate::time::uptime_ns() / 10_000_000;
            let mut buf = [0u8; 32];
            buf[0..8].copy_from_slice(&t.to_le_bytes());
            p.copy_to_user(a[0], &buf)?;
            Ok(t as usize)
        }
        165 => {
            // getrusage
            let mut buf = [0u8; 144];
            p.copy_to_user(a[1], &mut buf)?;
            Ok(0)
        }
        98 => {
            // futex
            crate::task::futex(p, a[0], a[1] as i32, a[2] as u32, a[3], a[5])
        }
        73 => {
            // ppoll
            sys_ppoll(p, a[0], a[1], a[2], a[3])
        }
        72 => {
            // pselect6
            let mut timeout_ms: i64 = -1;
            if a[2] != 0 {
                let mut b = [0u8; 16];
                p.copy_from_user(a[2], &mut b)?;
                let s = i64::from_le_bytes(b[0..8].try_into().unwrap());
                let ns = i64::from_le_bytes(b[8..16].try_into().unwrap());
                timeout_ms = s * 1000 + ns / 1_000_000;
            }
            sys_poll_common(p, a[0], a[1], timeout_ms)
        }
        19 => {
            // eventfd2
            let e = EventFd::new(a[1] as u32 & 1 != 0);
            let f = File::new(
                FileObj::EventFd(e),
                a[1] as u32 & O_NONBLOCK,
                a[1] as u32 & O_CLOEXEC != 0,
            );
            Ok(p.files.alloc(f, 0)? as usize)
        }
        20 => {
            let e = Epoll::new();
            let f = File::new(
                FileObj::Epoll(e),
                O_RDONLY | (if a[0] as u32 & O_NONBLOCK != 0 { O_NONBLOCK } else { 0 }),
                a[0] as u32 & O_CLOEXEC != 0,
            );
            Ok(p.files.alloc(f, 0)? as usize)
        }
        21 => {
            let ep = match &*p.files.get(a[0] as i32)?.obj.lock() {
                FileObj::Epoll(e) => e.clone(),
                _ => return Err(EINVAL),
            };
            let f = p.files.get(a[2] as i32)?;
            ep.ctl(a[1] as i32, a[2] as i32, f, a[3] as u32, a[4] as u64)?;
            Ok(0)
        }
        22 => sys_epoll_pwait(p, a),
        85..=87 | 107..=111 => Err(ENOSYS), // timerfd / POSIX timers

        // ---- sockets ----
        198 => socket::sys_socket(p, a[0], a[1] as u32, a[2] as u32),
        199 => socket::sys_socketpair(p, a[0], a[1] as u32, a[2] as u32, a[3]),
        200 => socket::sys_bind(p, a[0] as i32, a[1], a[2]),
        201 => socket::sys_listen(p, a[0] as i32, a[1] as i32),
        202 => socket::sys_accept(p, a[0] as i32, a[1], a[2], false),
        242 => socket::sys_accept(p, a[0] as i32, a[1], a[2], true),
        203 => socket::sys_connect(p, a[0] as i32, a[1], a[2]),
        204 => socket::sys_getsockname(p, a[0] as i32, a[1], a[2]),
        205 => socket::sys_getpeername(p, a[0] as i32, a[1], a[2]),
        206 => socket::sys_sendto(p, a[0] as i32, a[1], a[2], a[3], a[4], a[5]),
        207 => socket::sys_recvfrom(p, a[0] as i32, a[1], a[2], a[3], a[4], a[5]),
        208 => Ok(0), // setsockopt
        209 => socket::sys_getsockopt(p, a[0] as i32, a[1], a[2], a[3], a[4]),
        210 => {
            // shutdown
            socket::sys_shutdown(p, a[0] as i32, a[1] as i32)?;
            Ok(0)
        }
        211 => socket::sys_sendmsg(p, a[0] as i32, a[1], a[2]),
        212 => socket::sys_recvmsg(p, a[0] as i32, a[1], a[2]),
        269 => socket::sys_sendmmsg(p, a[0] as i32, a[1], a[2], a[3]),
        243 => socket::sys_recvmmsg(p, a[0] as i32, a[1], a[2], a[3]),

        _ => {
            crate::println!("[syscall] unimplemented nr={} ({} {} {} {})", nr, a[0], a[1], a[2], a[3]);
            Err(ENOSYS)
        }
    }
}

fn clock_now(clockid: usize) -> (u64, u64) {
    let ns = match clockid {
        0 => crate::time::realtime_ns(), // CLOCK_REALTIME
        1 => crate::time::uptime_ns(),   // CLOCK_MONOTONIC
        4 => crate::time::uptime_ns(),   // CLOCK_MONOTONIC_RAW
        7 => crate::time::uptime_ns(),   // CLOCK_BOOTTIME
        _ => crate::time::realtime_ns(),
    };
    (ns / 1_000_000_000, ns % 1_000_000_000)
}

fn signal_pending(p: &mut Process) -> Option<bool> {
    if p.sig.pending & !p.sig.blocked != 0 {
        Some(true)
    } else {
        None
    }
}

fn write_all(p: &mut Process, f: &Arc<File>, buf: &[u8]) -> Result<usize, Errno> {
    let mut total = 0;
    while total < buf.len() {
        match f.write(&buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(EAGAIN) => {
                if total > 0 {
                    break;
                }
                if f.nonblocking() {
                    return Err(EAGAIN);
                }
                if p.sig.pending & !p.sig.blocked != 0 {
                    return Err(EINTR);
                }
                crate::task::sleep_ticks(1);
            }
            Err(e) => return Err(e),
        }
    }
    let _ = p;
    Ok(total)
}

fn resolve_dir(p: &mut Process, dirfd: i32, path: &str) -> Result<Arc<Inode>, Errno> {
    if path.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(p.cwd.clone());
    }
    let f = p.files.get(dirfd)?;
    f.inode_of().ok_or(ENOTDIR)
}

fn sys_openat(p: &mut Process, dirfd: i32, path_ptr: usize, flags: u32, mode: u32) -> Result<usize, Errno> {
    let path = p.read_cstr(path_ptr, 4096)?;
    let base = resolve_dir(p, dirfd, &path)?;
    let follow = flags & O_NOFOLLOW == 0;
    let mut node = match fs::lookup(&base, &path, follow) {
        Ok(n) => n,
        Err(ENOENT) if flags & O_CREAT != 0 => {
            let kind = if flags & O_DIRECTORY != 0 {
                Kind::Dir
            } else {
                Kind::File
            };
            fs::create_at(&base, &path, mode, kind)?
        }
        Err(e) => return Err(e),
    };
    if node.kind() == Kind::Symlink {
        // O_NOFOLLOW on a symlink: Linux returns ELOOP
        if flags & O_NOFOLLOW != 0 {
            return Err(ELOOP);
        }
        node = fs::lookup(&base, &path, true)?;
    }
    if flags & O_DIRECTORY != 0 && node.kind() != Kind::Dir {
        return Err(ENOTDIR);
    }
    let acc = flags & O_ACCMODE;
    if flags & O_TRUNC != 0 && acc != O_RDONLY && node.kind() == Kind::File {
        node.truncate(0)?;
    }
    let obj = match node.kind() {
        Kind::Dir => FileObj::Dir {
            inode: node.clone(),
            offset: 0,
        },
        Kind::CharDev => FileObj::CharDev {
            rdev: node.inner.lock().rdev,
        },
        Kind::FdLink(n) => {
            let f = p.files.get(n as i32)?;
            let fd = p.files.alloc(f, 0)?;
            return Ok(fd as usize);
        }
        Kind::Fifo => FileObj::Pipe {
            pipe: crate::fs::pipe::Pipe::new(),
            write_end: acc != O_RDONLY,
        },
        _ => FileObj::Regular {
            inode: node.clone(),
            offset: if flags & O_APPEND != 0 {
                node.size()
            } else {
                0
            },
        },
    };
    let f = File::new(obj, flags & (O_NONBLOCK | O_APPEND), flags & O_CLOEXEC != 0);
    Ok(p.files.alloc(f, 0)? as usize)
}

fn sys_getdents64(p: &mut Process, fd: i32, buf: usize, count: usize) -> Result<usize, Errno> {
    let f = p.files.get(fd)?;
    let mut out: Vec<u8> = Vec::new();
    loop {
        let off = match &*f.obj.lock() {
            FileObj::Dir { offset, .. } => *offset,
            _ => return Err(ENOTDIR),
        };
        let inode = f.inode_of().ok_or(ENOTDIR)?;
        let entry = match inode.readdir(off)? {
            Some(e) => e,
            None => break,
        };
        let (name, ino, dtype) = entry;
        let reclen = (19 + name.len() + 1 + 7) & !7;
        if out.len() + reclen > count {
            break;
        }
        let mut rec = alloc::vec![0u8; reclen];
        rec[0..8].copy_from_slice(&ino.to_le_bytes());
        rec[8..16].copy_from_slice(&(off + 1).to_le_bytes());
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        rec[18] = dtype;
        rec[19..19 + name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&rec);
        if let FileObj::Dir { offset, .. } = &mut *f.obj.lock() {
            *offset += 1;
        }
    }
    p.copy_to_user(buf, &out)?;
    Ok(out.len())
}

fn sys_rt_sigaction(p: &mut Process, a: &[usize; 6]) -> Result<usize, Errno> {
    let sig = a[0];
    if sig < 1 || sig > 64 {
        return Err(EINVAL);
    }
    if a[2] != 0 {
        let mut b = [0u8; 32];
        p.copy_from_user(a[2], &mut b)?;
        b[0..8].copy_from_slice(&(p.sig.actions[sig].handler as u64).to_le_bytes());
        b[8..16].copy_from_slice(&p.sig.actions[sig].flags.to_le_bytes());
        b[16..24].copy_from_slice(&(p.sig.actions[sig].restorer as u64).to_le_bytes());
        b[24..32].copy_from_slice(&p.sig.actions[sig].mask.to_le_bytes());
        p.copy_to_user(a[2], &b)?;
    }
    if a[1] != 0 {
        let mut b = [0u8; 32];
        p.copy_from_user(a[1], &mut b)?;
        let act = SigAction {
            handler: u64::from_le_bytes(b[0..8].try_into().unwrap()) as usize,
            flags: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            restorer: u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize,
            mask: u64::from_le_bytes(b[24..32].try_into().unwrap()),
        };
        p.sig.actions[sig] = act;
    }
    Ok(0)
}

fn sys_rt_sigprocmask(p: &mut Process, a: &[usize; 6]) -> Result<usize, Errno> {
    let how = a[0] as i32;
    if a[2] != 0 {
        write_u64(p, a[2], p.sig.blocked)?;
    }
    if a[1] != 0 {
        let set = read_u64(p, a[1])?;
        p.sig.blocked = match how {
            0 => set,             // SIG_BLOCK
            1 => p.sig.blocked & !set, // SIG_UNBLOCK
            2 => set,             // SIG_SETMASK
            _ => return Err(EINVAL),
        };
    }
    Ok(0)
}

fn sys_wait4(p: &mut Process, pid: i32, status_ptr: usize, options: u32, _rusage: usize) -> Result<usize, Errno> {
    loop {
        // find a zombie child
        let mut found: Option<(usize, i32)> = None;
        let children = p.children.clone();
        for c in children.iter() {
            if pid > 0 && *c != pid as usize {
                continue;
            }
            if let Some(code) = crate::task::child_exit_code(*c) {
                found = Some((*c, code));
                break;
            }
        }
        if let Some((cpid, code)) = found {
            p.children.retain(|x| *x != cpid);
            crate::task::reap_child(cpid);
            if status_ptr != 0 {
                write_u32(p, status_ptr, (code as u32 & 0xff) << 8)?;
            }
            return Ok(cpid);
        }
        if p.children.is_empty() {
            return Err(ECHILD);
        }
        if options & WNOHANG != 0 {
            return Ok(0);
        }
        if p.sig.pending & !p.sig.blocked != 0 {
            return Err(EINTR);
        }
        p.parent_waiting = true;
        crate::task::sleep_ticks(1);
    }
}

fn read_str_array(p: &mut Process, ptr: usize) -> Result<Vec<String>, Errno> {
    let mut out = Vec::new();
    if ptr == 0 {
        return Ok(out);
    }
    for i in 0..1024 {
        let mut b = [0u8; 8];
        p.copy_from_user(ptr + i * 8, &mut b)?;
        let sp = u64::from_le_bytes(b) as usize;
        if sp == 0 {
            break;
        }
        out.push(p.read_cstr(sp, 4096)?);
    }
    Ok(out)
}

fn read_iovs(p: &mut Process, iov: usize, cnt: usize) -> Result<Vec<(usize, usize)>, Errno> {
    let mut out = Vec::new();
    for i in 0..cnt.min(1024) {
        let mut b = [0u8; 16];
        p.copy_from_user(iov + i * 16, &mut b)?;
        let base = u64::from_le_bytes(b[0..8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(b[8..16].try_into().unwrap()) as usize;
        out.push((base, len));
    }
    Ok(out)
}

fn write_u32(p: &mut Process, va: usize, v: u32) -> Result<(), Errno> {
    if va == 0 {
        return Err(EFAULT);
    }
    p.copy_to_user(va, &v.to_le_bytes())
}

fn write_u64(p: &mut Process, va: usize, v: u64) -> Result<(), Errno> {
    if va == 0 {
        return Err(EFAULT);
    }
    p.copy_to_user(va, &v.to_le_bytes())
}

fn read_u64(p: &mut Process, va: usize) -> Result<u64, Errno> {
    let mut b = [0u8; 8];
    p.copy_from_user(va, &mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn write_stat(p: &mut Process, va: usize, st: &fs::Stat) -> Result<(), Errno> {
    let mut b = [0u8; 128];
    b[0..8].copy_from_slice(&st.dev.to_le_bytes());
    b[8..16].copy_from_slice(&st.ino.to_le_bytes());
    b[16..20].copy_from_slice(&st.mode.to_le_bytes());
    b[20..24].copy_from_slice(&st.nlink.to_le_bytes());
    b[24..28].copy_from_slice(&st.uid.to_le_bytes());
    b[28..32].copy_from_slice(&st.gid.to_le_bytes());
    b[32..40].copy_from_slice(&st.rdev.to_le_bytes());
    b[48..56].copy_from_slice(&st.size.to_le_bytes());
    b[56..60].copy_from_slice(&(st.blksize as i32).to_le_bytes());
    b[64..72].copy_from_slice(&st.blocks.to_le_bytes());
    b[72..80].copy_from_slice(&st.atime.to_le_bytes());
    b[88..96].copy_from_slice(&st.mtime.to_le_bytes());
    b[104..112].copy_from_slice(&st.ctime.to_le_bytes());
    p.copy_to_user(va, &b)
}

/// epoll_pwait(epfd, events, maxevents, timeout, sigmask, sigsetsize)
fn sys_epoll_pwait(p: &mut Process, a: &[usize; 6]) -> Result<usize, Errno> {
    let ep = match &*p.files.get(a[0] as i32)?.obj.lock() {
        FileObj::Epoll(e) => e.clone(),
        _ => return Err(EINVAL),
    };
    let maxevents = a[2];
    let timeout = a[3] as i64;
    let saved_mask = if a[4] != 0 {
        let m = read_u64(p, a[4])?;
        let old = p.sig.blocked;
        p.sig.blocked = m;
        Some(old)
    } else {
        None
    };
    let start = crate::time::ticks();
    let result = loop {
        let items: Vec<(i32, u32, u64)> = ep
            .items
            .lock()
            .iter()
            .map(|i| (i.fd, i.events, i.data))
            .collect();
        let mut n = 0;
        for (fd, events, data) in items.iter() {
            let file = match p.files.get_opt(*fd) {
                Some(f) => f,
                None => continue,
            };
            let ready = file.readiness();
            if ready & (*events | EPOLLERR | EPOLLHUP) != 0 {
                if n >= maxevents {
                    break;
                }
                let mut ev = [0u8; 12];
                ev[0..4].copy_from_slice(&(ready & (events | EPOLLERR | EPOLLHUP)).to_le_bytes());
                ev[4..12].copy_from_slice(&data.to_le_bytes());
                p.copy_to_user(a[1] + n * 12, &ev)?;
                n += 1;
            }
        }
        if n > 0 {
            break Ok(n);
        }
        if timeout >= 0 && (crate::time::ticks() - start) as i64 >= (timeout / 4).max(1) {
            break Ok(0);
        }
        if p.sig.pending & !p.sig.blocked != 0 {
            break Err(EINTR);
        }
        crate::task::sleep_ticks(1);
    };
    if let Some(m) = saved_mask {
        p.sig.blocked = m;
    }
    result
}

/// ppoll(fds, nfds, tmo_p, sigmask, sigsetsize)
fn sys_ppoll(p: &mut Process, fds: usize, nfds: usize, tmo: usize, sigmask: usize) -> Result<usize, Errno> {
    let mut timeout_ms: i64 = -1;
    if tmo != 0 {
        let mut b = [0u8; 16];
        p.copy_from_user(tmo, &mut b)?;
        let s = i64::from_le_bytes(b[0..8].try_into().unwrap());
        let ns = i64::from_le_bytes(b[8..16].try_into().unwrap());
        timeout_ms = s * 1000 + ns / 1_000_000;
    }
    let saved_mask = if sigmask != 0 {
        let m = read_u64(p, sigmask)?;
        let old = p.sig.blocked;
        p.sig.blocked = m;
        Some(old)
    } else {
        None
    };
    let r = sys_poll_common(p, fds, nfds, timeout_ms);
    if let Some(m) = saved_mask {
        p.sig.blocked = m;
    }
    r
}

fn sys_poll_common(p: &mut Process, fds: usize, nfds: usize, timeout_ms: i64) -> Result<usize, Errno> {
    const POLLIN: i16 = 0x001;
    const POLLOUT: i16 = 0x004;
    const POLLERR: i16 = 0x008;
    const POLLHUP: i16 = 0x010;
    let start = crate::time::ticks();
    loop {
        let mut n = 0;
        for i in 0..nfds {
            let mut b = [0u8; 8];
            p.copy_from_user(fds + i * 8, &mut b)?;
            let fd = i32::from_le_bytes(b[0..4].try_into().unwrap());
            let events = i16::from_le_bytes(b[4..6].try_into().unwrap());
            let file = match p.files.get_opt(fd) {
                Some(f) => f,
                None => {
                    p.copy_to_user(fds + i * 8 + 6, &POLLERR.to_le_bytes())?;
                    continue;
                }
            };
            let ready = file.readiness();
            let mut rev: i16 = 0;
            if events & POLLIN != 0 && ready & EPOLLIN != 0 {
                rev |= POLLIN;
            }
            if events & POLLOUT != 0 && ready & EPOLLOUT != 0 {
                rev |= POLLOUT;
            }
            if ready & EPOLLERR != 0 {
                rev |= POLLERR;
            }
            if ready & EPOLLHUP != 0 {
                rev |= POLLHUP;
            }
            p.copy_to_user(fds + i * 8 + 6, &rev.to_le_bytes())?;
            if rev != 0 {
                n += 1;
            }
        }
        if n > 0 {
            return Ok(n);
        }
        if timeout_ms == 0 {
            return Ok(0);
        }
        if timeout_ms > 0 && (crate::time::ticks() - start) as i64 >= (timeout_ms / 4).max(1) {
            return Ok(0);
        }
        if p.sig.pending & !p.sig.blocked != 0 {
            return Err(EINTR);
        }
        crate::task::sleep_ticks(1);
    }
}

pub fn deliver_signal_fault(tf: &mut TrapFrame, sig: usize) {
    let t = crate::task::current();
    if let Some(p) = t.process.as_mut() {
        p.sig.raise_with(
            sig,
            SigInfo {
                code: 1,
                addr: crate::csr::stval(),
                ..Default::default()
            },
        );
        if p.sig.is_default(sig) {
            crate::println!(
                "[fault] pid {} killed by signal {} at pc={:#x} addr={:#x}",
                p.pid,
                sig,
                tf.sepc,
                crate::csr::stval()
            );
            crate::task::exit_current(128 + sig as i32);
        }
    }
    let _ = tf;
}
