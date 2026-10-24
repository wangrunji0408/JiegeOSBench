use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::TrapFrame;
use crate::memory::{self, PTE_R, PTE_W, PTE_X};

const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const ECHILD: i32 = 10;
const EAGAIN: i32 = 11;
const ENOMEM: i32 = 12;
const EACCES: i32 = 13;
const EFAULT: i32 = 14;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;
const ENOSYS: i32 = 38;
const AT_FDCWD: i32 = -100;

static SEEN: Mutex<[u64; 16]> = Mutex::new([0; 16]);

type SysResult = Result<usize, i32>;

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timeval {
    seconds: i64,
    microseconds: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rlimit {
    current: u64,
    maximum: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Utsname {
    sysname: [u8; 65],
    nodename: [u8; 65],
    release: [u8; 65],
    version: [u8; 65],
    machine: [u8; 65],
    domainname: [u8; 65],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxStat {
    device: u64,
    inode: u64,
    mode: u32,
    links: u32,
    uid: u32,
    gid: u32,
    rdevice: u64,
    pad1: u64,
    size: i64,
    block_size: i32,
    pad2: i32,
    blocks: i64,
    atime: i64,
    atime_nsec: u64,
    mtime: i64,
    mtime_nsec: u64,
    ctime: i64,
    ctime_nsec: u64,
    unused4: u32,
    unused5: u32,
}

pub fn dispatch(frame: &mut TrapFrame) {
    let number = frame.regs[17];
    let arguments = [
        frame.regs[10],
        frame.regs[11],
        frame.regs[12],
        frame.regs[13],
        frame.regs[14],
        frame.regs[15],
    ];
    trace(number, &arguments);
    let result = match number {
        17 => getcwd(arguments[0], arguments[1]),
        20 => crate::fd::epoll_create().try_into().map_err(|_| EINVAL),
        21 => epoll_ctl(arguments),
        22 => epoll_wait(arguments),
        23 => crate::fd::duplicate(arguments[0] as i32, None).map(|fd| fd as usize),
        24 => dup3(arguments),
        25 => fcntl(arguments),
        29 => ioctl(arguments),
        34 => mkdirat(arguments),
        35 => unlinkat(arguments),
        46 => ftruncate(arguments),
        48 => faccessat(arguments),
        49 => chdir(arguments[0]),
        52 => crate::fd::metadata(arguments[0] as i32).map(|_| 0),
        53 => fchmodat(arguments),
        54 => fchownat(arguments),
        55 => crate::fd::metadata(arguments[0] as i32).map(|_| 0),
        56 => openat(arguments),
        57 => crate::fd::close(arguments[0] as i32).map(|_| 0),
        61 => getdents64(arguments),
        62 => crate::fd::seek(arguments[0] as i32, arguments[1] as i64, arguments[2] as u32)
            .map(|offset| offset as usize),
        63 => read(arguments),
        64 => write(arguments),
        65 => readv(arguments),
        66 => writev(arguments),
        67 => pread64(arguments),
        68 => pwrite64(arguments),
        78 => readlinkat(arguments),
        79 => newfstatat(arguments),
        80 => fstat(arguments),
        82 | 83 => Ok(0),
        88 => utimensat(arguments),
        93 | 94 => exit(arguments[0]),
        96 => Ok(1),
        98 => futex(arguments),
        99 | 100 => Ok(0),
        101 => nanosleep(arguments[0]),
        113 => clock_gettime(arguments[0], arguments[1]),
        114 => clock_getres(arguments[1]),
        115 => clock_nanosleep(arguments[2]),
        123 => sched_getaffinity(arguments),
        124 => Ok(0),
        129 => Ok(0),
        132 => sigaltstack(arguments),
        134 => rt_sigaction(arguments),
        135 => rt_sigprocmask(arguments),
        136 => Ok(0),
        160 => uname(arguments[0]),
        163 => getrlimit(arguments[0], arguments[1]),
        164 => Ok(0),
        165 => zero_user(arguments[1], 144),
        169 => gettimeofday(arguments[0]),
        172 => Ok(1),
        173 => Ok(0),
        174 | 175 | 176 | 177 => Ok(0),
        178 => Ok(1),
        179 => sysinfo(arguments[0]),
        198 => crate::fd::socket(arguments[0] as u32, arguments[1] as u32, arguments[2] as u32)
            .map(|fd| fd as usize),
        199 => Err(ENOSYS),
        200 => socket_bind(arguments),
        201 => crate::fd::listen(arguments[0] as i32).map(|_| 0),
        202 => socket_accept(arguments, 0),
        203 => Err(ENOSYS),
        204 => socket_name(arguments, true),
        205 => socket_name(arguments, false),
        206 => write(arguments),
        207 => read(arguments),
        208 => crate::fd::set_socket_option(
            arguments[0] as i32,
            arguments[1] as u32,
            arguments[2] as u32,
        )
        .map(|_| 0),
        209 => socket_get_option(arguments),
        210 => crate::fd::shutdown(arguments[0] as i32).map(|_| 0),
        211 | 212 => Err(ENOSYS),
        214 => brk(arguments[0]),
        215 => munmap(arguments[0], arguments[1]),
        220 | 221 => Err(ENOSYS),
        222 => mmap(arguments),
        223 => Ok(0),
        226 => mprotect(arguments),
        227 | 228 | 229 | 233 => Ok(0),
        242 => socket_accept(arguments, arguments[3] as u32),
        258 | 259 => Err(ENOSYS),
        260 => Err(ECHILD),
        261 => prlimit64(arguments),
        278 => getrandom(arguments),
        293 => Err(ENOSYS),
        _ => {
            crate::println!("linux: unsupported syscall {} args={arguments:#x?}", number);
            Err(ENOSYS)
        }
    };
    match result {
        Ok(value) => frame.regs[10] = value,
        Err(error) => frame.regs[10] = (-error) as usize,
    }
}

fn trace(number: usize, arguments: &[usize; 6]) {
    if number >= 1024 {
        return;
    }
    let mut seen = SEEN.lock();
    let word = number / 64;
    let bit = 1u64 << (number % 64);
    if seen[word] & bit == 0 {
        seen[word] |= bit;
        crate::println!("linux: first syscall {number} args={arguments:#x?}");
    }
}

fn copy_to_user<T: Copy>(address: usize, value: &T) -> SysResult {
    memory::write_user(address, value).map(|_| 0).map_err(|_| EFAULT)
}

fn read_string(address: usize) -> Result<String, i32> {
    let bytes = memory::read_user_string(address, 4096).map_err(|_| EFAULT)?;
    String::from_utf8(bytes).map_err(|_| EINVAL)
}

fn path_at(dirfd: i32, address: usize) -> Result<String, i32> {
    let path = read_string(address)?;
    if path.starts_with('/') {
        return Ok(path);
    }
    if dirfd == AT_FDCWD {
        return Ok(format!("/{path}"));
    }
    let directory = crate::fd::path(dirfd)?;
    Ok(format!("{}/{path}", directory.trim_end_matches('/')))
}

fn checked_buffer(length: usize) -> Result<(), i32> {
    if length > 64 * 1024 * 1024 {
        Err(ENOMEM)
    } else {
        Ok(())
    }
}

fn read(arguments: [usize; 6]) -> SysResult {
    checked_buffer(arguments[2])?;
    let mut buffer = vec![0; arguments[2]];
    let count = crate::fd::read(arguments[0] as i32, &mut buffer)?;
    memory::copy_to_user(arguments[1], &buffer[..count]).map_err(|_| EFAULT)?;
    Ok(count)
}

fn write(arguments: [usize; 6]) -> SysResult {
    checked_buffer(arguments[2])?;
    let mut buffer = vec![0; arguments[2]];
    memory::copy_from_user(arguments[1], &mut buffer).map_err(|_| EFAULT)?;
    crate::fd::write(arguments[0] as i32, &buffer)
}

fn pread64(arguments: [usize; 6]) -> SysResult {
    checked_buffer(arguments[2])?;
    let mut buffer = vec![0; arguments[2]];
    let count = crate::fd::read_at(
        arguments[0] as i32,
        arguments[3] as u64,
        &mut buffer,
    )?;
    memory::copy_to_user(arguments[1], &buffer[..count]).map_err(|_| EFAULT)?;
    Ok(count)
}

fn pwrite64(arguments: [usize; 6]) -> SysResult {
    checked_buffer(arguments[2])?;
    let mut buffer = vec![0; arguments[2]];
    memory::copy_from_user(arguments[1], &mut buffer).map_err(|_| EFAULT)?;
    crate::fd::write_at(
        arguments[0] as i32,
        arguments[3] as u64,
        &buffer,
    )
}

fn readv(arguments: [usize; 6]) -> SysResult {
    let mut total = 0;
    for index in 0..arguments[2] {
        let base = memory::read_user::<u64>(arguments[1] + index * 16).map_err(|_| EFAULT)?;
        let length = memory::read_user::<usize>(arguments[1] + index * 16 + 8)
            .map_err(|_| EFAULT)?;
        checked_buffer(length)?;
        let mut buffer = vec![0; length];
        let count = crate::fd::read(arguments[0] as i32, &mut buffer)?;
        memory::copy_to_user(base as usize, &buffer[..count]).map_err(|_| EFAULT)?;
        total += count;
        if count < length {
            break;
        }
    }
    Ok(total)
}

fn writev(arguments: [usize; 6]) -> SysResult {
    let mut total = 0;
    for index in 0..arguments[2] {
        let base = memory::read_user::<u64>(arguments[1] + index * 16).map_err(|_| EFAULT)?;
        let length = memory::read_user::<usize>(arguments[1] + index * 16 + 8)
            .map_err(|_| EFAULT)?;
        checked_buffer(length)?;
        let mut buffer = vec![0; length];
        memory::copy_from_user(base as usize, &mut buffer).map_err(|_| EFAULT)?;
        let count = crate::fd::write(arguments[0] as i32, &buffer)?;
        total += count;
        if count < length {
            break;
        }
    }
    Ok(total)
}

fn openat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    crate::fd::open(&path, arguments[2] as u32, arguments[3] as u32)
        .map(|fd| fd as usize)
}

fn getcwd(address: usize, size: usize) -> SysResult {
    if size < 2 {
        return Err(EINVAL);
    }
    memory::copy_to_user(address, b"/\0").map_err(|_| EFAULT)?;
    Ok(2)
}

fn chdir(address: usize) -> SysResult {
    let path = read_string(address)?;
    let metadata = crate::fs::metadata(&path, true)?;
    if metadata.mode & 0o170000 != 0o040000 {
        return Err(ENOTDIR);
    }
    Ok(0)
}

fn mkdirat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    crate::fs::mkdir(&path, arguments[2] as u32).map(|_| 0)
}

fn unlinkat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    crate::fs::unlink(&path).map(|_| 0)
}

fn faccessat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    crate::fs::metadata(&path, true).map(|_| 0).map_err(|error| {
        if error == EACCES { EACCES } else { error }
    })
}

fn utimensat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    crate::fs::metadata(&path, arguments[3] & 0x100 == 0)
        .map(|_| 0)
}

fn ftruncate(arguments: [usize; 6]) -> SysResult {
    let metadata = crate::fd::metadata(arguments[0] as i32)?;
    if metadata.size == arguments[1] as u64 {
        Ok(0)
    } else {
        Err(ENOSYS)
    }
}

fn linux_stat(metadata: crate::fs::Metadata) -> LinuxStat {
    LinuxStat {
        device: 1,
        inode: metadata.inode,
        mode: metadata.mode,
        links: 1,
        uid: 0,
        gid: 0,
        rdevice: 0,
        pad1: 0,
        size: metadata.size as i64,
        block_size: 4096,
        pad2: 0,
        blocks: ((metadata.size + 511) / 512) as i64,
        atime: 0,
        atime_nsec: 0,
        mtime: 0,
        mtime_nsec: 0,
        ctime: 0,
        ctime_nsec: 0,
        unused4: 0,
        unused5: 0,
    }
}

fn newfstatat(arguments: [usize; 6]) -> SysResult {
    let metadata = if arguments[1] == 0 && arguments[3] & 0x1000 != 0 {
        crate::fd::metadata(arguments[0] as i32)?
    } else {
        let path = path_at(arguments[0] as i32, arguments[1])?;
        crate::fs::metadata(&path, arguments[3] & 0x100 == 0)?
    };
    copy_to_user(arguments[2], &linux_stat(metadata))
}

fn fstat(arguments: [usize; 6]) -> SysResult {
    let metadata = crate::fd::metadata(arguments[0] as i32)?;
    copy_to_user(arguments[1], &linux_stat(metadata))
}

fn readlinkat(arguments: [usize; 6]) -> SysResult {
    let path = path_at(arguments[0] as i32, arguments[1])?;
    if path != "/proc/self/exe" {
        return Err(ENOENT);
    }
    let target = b"/usr/sbin/nginx";
    let count = target.len().min(arguments[3]);
    memory::copy_to_user(arguments[2], &target[..count]).map_err(|_| EFAULT)?;
    Ok(count)
}

fn append_dirent(
    output: &mut Vec<u8>,
    index: usize,
    name: &str,
    metadata: crate::fs::Metadata,
) -> usize {
    let record_length = memory::align_up(19 + name.len() + 1, 8);
    let start = output.len();
    output.resize(start + record_length, 0);
    output[start..start + 8].copy_from_slice(&metadata.inode.to_le_bytes());
    output[start + 8..start + 16].copy_from_slice(&((index + 1) as i64).to_le_bytes());
    output[start + 16..start + 18].copy_from_slice(&(record_length as u16).to_le_bytes());
    output[start + 18] = match metadata.mode & 0o170000 {
        0o040000 => 4,
        0o120000 => 10,
        _ => 8,
    };
    output[start + 19..start + 19 + name.len()].copy_from_slice(name.as_bytes());
    record_length
}

fn getdents64(arguments: [usize; 6]) -> SysResult {
    let fd = arguments[0] as i32;
    let entries = crate::fd::directory_entries(fd)?;
    let mut index = crate::fd::directory_offset(fd)? as usize;
    let mut output = Vec::new();
    while index < entries.len() {
        let (name, metadata) = &entries[index];
        let record_length = memory::align_up(19 + name.len() + 1, 8);
        if output.len() + record_length > arguments[2] {
            break;
        }
        append_dirent(&mut output, index, name, *metadata);
        index += 1;
    }
    if output.is_empty() && index < entries.len() {
        return Err(EINVAL);
    }
    memory::copy_to_user(arguments[1], &output).map_err(|_| EFAULT)?;
    crate::fd::set_directory_offset(fd, index as u64)?;
    Ok(output.len())
}

fn fcntl(arguments: [usize; 6]) -> SysResult {
    crate::fd::fcntl(
        arguments[0] as i32,
        arguments[1] as u32,
        arguments[2] as u32,
    )
    .map(|value| value as usize)
}

fn dup3(arguments: [usize; 6]) -> SysResult {
    if arguments[0] == arguments[1] {
        return Err(EINVAL);
    }
    crate::fd::duplicate(arguments[0] as i32, Some(arguments[1] as i32))
        .map(|fd| fd as usize)
}

fn ioctl(arguments: [usize; 6]) -> SysResult {
    match arguments[1] as u32 {
        0x5413 => {
            let size = [24u16, 80, 0, 0];
            memory::copy_to_user(
                arguments[2],
                unsafe {
                    core::slice::from_raw_parts(size.as_ptr().cast::<u8>(), size_of_val(&size))
                },
            )
            .map_err(|_| EFAULT)?;
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}

fn brk(address: usize) -> SysResult {
    crate::process::with_state(|process| {
        if address == 0 {
            return Ok(process.program_break);
        }
        if address < process.program_break_base {
            return Ok(process.program_break);
        }
        let old_end = memory::align_up(process.program_break, memory::PAGE_SIZE);
        let new_end = memory::align_up(address, memory::PAGE_SIZE);
        if new_end > old_end {
            memory::map_user(old_end, new_end - old_end, PTE_R | PTE_W).map_err(|_| ENOMEM)?;
        } else if new_end < old_end {
            let minimum = memory::align_up(process.program_break_base, memory::PAGE_SIZE);
            let unmap_start = new_end.max(minimum);
            if unmap_start < old_end {
                memory::unmap_user(unmap_start, old_end - unmap_start).map_err(|_| ENOMEM)?;
            }
        }
        process.program_break = address;
        Ok(address)
    })
}

fn mmap(arguments: [usize; 6]) -> SysResult {
    let length = memory::align_up(arguments[1], memory::PAGE_SIZE);
    if length == 0 {
        return Err(EINVAL);
    }
    let flags = arguments[3] as u32;
    let mut protection = 0;
    if arguments[2] & 1 != 0 {
        protection |= PTE_R;
    }
    if arguments[2] & 2 != 0 {
        protection |= PTE_W;
    }
    if arguments[2] & 4 != 0 {
        protection |= PTE_X;
    }
    let fixed = flags & 0x10 != 0;
    let address = if fixed {
        if arguments[0] & (memory::PAGE_SIZE - 1) != 0 || arguments[0] < 0x10000 {
            return Err(EINVAL);
        }
        arguments[0]
    } else {
        crate::process::with_state(|process| {
            let address = process.mmap_next;
            process.mmap_next = process.mmap_next.checked_add(length).ok_or(ENOMEM)?;
            Ok::<usize, i32>(address)
        })?
    };
    if fixed {
        memory::unmap_user(address, length).map_err(|_| EPERM)?;
    }
    memory::map_user(address, length, protection).map_err(|_| ENOMEM)?;
    if flags & 0x20 == 0 {
        let mut offset = 0usize;
        while offset < arguments[1] {
            let count = (arguments[1] - offset).min(1024 * 1024);
            let mut buffer = vec![0; count];
            let read = crate::fd::read_at(
                arguments[4] as i32,
                (arguments[5] + offset) as u64,
                &mut buffer,
            )?;
            if read == 0 {
                break;
            }
            memory::loader_write(address + offset, &buffer[..read]).map_err(|_| EFAULT)?;
            offset += read;
            if read < count {
                break;
            }
        }
    }
    Ok(address)
}

fn munmap(address: usize, length: usize) -> SysResult {
    if address & (memory::PAGE_SIZE - 1) != 0 {
        return Err(EINVAL);
    }
    memory::unmap_user(address, length).map(|_| 0).map_err(|_| EINVAL)
}

fn mprotect(arguments: [usize; 6]) -> SysResult {
    let mut protection = 0;
    if arguments[2] & 1 != 0 {
        protection |= PTE_R;
    }
    if arguments[2] & 2 != 0 {
        protection |= PTE_W;
    }
    if arguments[2] & 4 != 0 {
        protection |= PTE_X;
    }
    memory::protect_user(arguments[0], arguments[1], protection)
        .map(|_| 0)
        .map_err(|_| ENOMEM)
}

fn current_timespec() -> Timespec {
    let ticks = crate::arch::time();
    Timespec {
        seconds: (ticks / 10_000_000) as i64,
        nanoseconds: ((ticks % 10_000_000) * 100) as i64,
    }
}

fn realtime_timespec() -> Timespec {
    let mut time = current_timespec();
    time.seconds += env!("JIEGE_UNIX_EPOCH").parse::<i64>().unwrap();
    time
}

fn clock_gettime(clock: usize, address: usize) -> SysResult {
    if clock == 0 {
        copy_to_user(address, &realtime_timespec())
    } else {
        copy_to_user(address, &current_timespec())
    }
}

fn clock_getres(address: usize) -> SysResult {
    if address == 0 {
        return Ok(0);
    }
    copy_to_user(
        address,
        &Timespec {
            seconds: 0,
            nanoseconds: 1_000_000,
        },
    )
}

fn nanosleep(address: usize) -> SysResult {
    let requested: Timespec = memory::read_user(address).map_err(|_| EFAULT)?;
    if requested.seconds < 0 || !(0..1_000_000_000).contains(&requested.nanoseconds) {
        return Err(EINVAL);
    }
    let duration = (requested.seconds as u64)
        .saturating_mul(10_000_000)
        .saturating_add(requested.nanoseconds as u64 / 100);
    let deadline = crate::arch::time().saturating_add(duration);
    crate::arch::enable_interrupts();
    while crate::arch::time() < deadline {
        crate::arch::wait_for_interrupt();
    }
    crate::arch::disable_interrupts();
    Ok(0)
}

fn clock_nanosleep(address: usize) -> SysResult {
    nanosleep(address)
}

fn gettimeofday(address: usize) -> SysResult {
    if address == 0 {
        return Ok(0);
    }
    let time = realtime_timespec();
    copy_to_user(
        address,
        &Timeval {
            seconds: time.seconds,
            microseconds: time.nanoseconds / 1000,
        },
    )
}

fn zero_user(address: usize, length: usize) -> SysResult {
    let zeros = vec![0; length];
    memory::copy_to_user(address, &zeros).map_err(|_| EFAULT)?;
    Ok(0)
}

fn getrlimit(resource: usize, address: usize) -> SysResult {
    let limit = match resource as u32 {
        3 => Rlimit {
            current: 8 * 1024 * 1024,
            maximum: 8 * 1024 * 1024,
        },
        7 => Rlimit {
            current: 1024,
            maximum: 1024,
        },
        _ => Rlimit {
            current: u64::MAX,
            maximum: u64::MAX,
        },
    };
    copy_to_user(address, &limit)
}

fn prlimit64(arguments: [usize; 6]) -> SysResult {
    if arguments[2] != 0 {
        let _: Rlimit = memory::read_user(arguments[2]).map_err(|_| EFAULT)?;
    }
    if arguments[3] != 0 {
        getrlimit(arguments[1], arguments[3])?;
    }
    Ok(0)
}

fn uname(address: usize) -> SysResult {
    let mut value: Utsname = unsafe { core::mem::zeroed() };
    let fields = [
        (&mut value.sysname, &b"Linux"[..]),
        (&mut value.nodename, &b"jiege-riscv"[..]),
        (&mut value.release, &b"6.12.0-jiege"[..]),
        (&mut value.version, &b"#1 Rust RISC-V"[..]),
        (&mut value.machine, &b"riscv64"[..]),
        (&mut value.domainname, &b"local"[..]),
    ];
    for (destination, source) in fields {
        destination[..source.len()].copy_from_slice(source);
    }
    copy_to_user(address, &value)
}

fn sysinfo(address: usize) -> SysResult {
    let mut data = vec![0u8; 112];
    data[32..40].copy_from_slice(&(512u64 * 1024 * 1024).to_le_bytes());
    data[40..48].copy_from_slice(&(256u64 * 1024 * 1024).to_le_bytes());
    data[80..82].copy_from_slice(&1u16.to_le_bytes());
    data[104..108].copy_from_slice(&1u32.to_le_bytes());
    memory::copy_to_user(address, &data).map_err(|_| EFAULT)?;
    Ok(0)
}

fn sched_getaffinity(arguments: [usize; 6]) -> SysResult {
    if arguments[1] < 8 {
        return Err(EINVAL);
    }
    memory::copy_to_user(arguments[2], &1u64.to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(8)
}

fn rt_sigaction(arguments: [usize; 6]) -> SysResult {
    if arguments[2] != 0 {
        zero_user(arguments[2], 32)?;
    }
    Ok(0)
}

fn rt_sigprocmask(arguments: [usize; 6]) -> SysResult {
    if arguments[1] != 0 {
        zero_user(arguments[1], 8)?;
    }
    Ok(0)
}

fn sigaltstack(arguments: [usize; 6]) -> SysResult {
    if arguments[1] != 0 {
        zero_user(arguments[1], 24)?;
    }
    Ok(0)
}

fn futex(arguments: [usize; 6]) -> SysResult {
    match (arguments[1] as u32) & 0x7f {
        0 => Err(EAGAIN),
        1 => Ok(0),
        _ => Err(ENOSYS),
    }
}

fn getrandom(arguments: [usize; 6]) -> SysResult {
    checked_buffer(arguments[1])?;
    let mut buffer = vec![0; arguments[1]];
    let mut state = crate::arch::time() ^ arguments[0] as u64 ^ 0x9e37_79b9_7f4a_7c15;
    for byte in &mut buffer {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    memory::copy_to_user(arguments[0], &buffer).map_err(|_| EFAULT)?;
    Ok(buffer.len())
}

fn epoll_ctl(arguments: [usize; 6]) -> SysResult {
    let interest = if arguments[3] == 0 {
        None
    } else {
        Some(crate::fd::EpollInterest {
            events: memory::read_user::<u32>(arguments[3]).map_err(|_| EFAULT)?,
            data: memory::read_user::<u64>(arguments[3] + 8).map_err(|_| EFAULT)?,
        })
    };
    crate::fd::epoll_ctl(
        arguments[0] as i32,
        arguments[1] as u32,
        arguments[2] as i32,
        interest,
    )
    .map(|_| 0)
}

fn epoll_wait(arguments: [usize; 6]) -> SysResult {
    if arguments[2] == 0 {
        return Err(EINVAL);
    }
    let results = crate::fd::epoll_wait(
        arguments[0] as i32,
        arguments[2],
        arguments[3] as i32,
    )?;
    let mut output = Vec::new();
    for result in &results {
        output.extend_from_slice(&result.events.to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(&result.data.to_le_bytes());
    }
    memory::copy_to_user(arguments[1], &output).map_err(|_| EFAULT)?;
    Ok(results.len())
}

fn read_sockaddr(address: usize) -> Result<([u8; 4], u16), i32> {
    let sockaddr: [u8; 16] = memory::read_user(address).map_err(|_| EFAULT)?;
    if u16::from_le_bytes([sockaddr[0], sockaddr[1]]) != 2 {
        return Err(EINVAL);
    }
    Ok((
        [sockaddr[4], sockaddr[5], sockaddr[6], sockaddr[7]],
        u16::from_be_bytes([sockaddr[2], sockaddr[3]]),
    ))
}

fn write_sockaddr(
    address: usize,
    length_address: usize,
    endpoint: Option<smoltcp::wire::IpEndpoint>,
) -> SysResult {
    if address == 0 {
        return Ok(0);
    }
    let supplied = memory::read_user::<u32>(length_address).map_err(|_| EFAULT)? as usize;
    let mut sockaddr = [0u8; 16];
    sockaddr[..2].copy_from_slice(&2u16.to_le_bytes());
    if let Some(endpoint) = endpoint {
        sockaddr[2..4].copy_from_slice(&endpoint.port.to_be_bytes());
        let smoltcp::wire::IpAddress::Ipv4(address) = endpoint.addr;
        sockaddr[4..8].copy_from_slice(&address.octets());
    }
    let count = supplied.min(sockaddr.len());
    memory::copy_to_user(address, &sockaddr[..count]).map_err(|_| EFAULT)?;
    memory::write_user(length_address, &(sockaddr.len() as u32)).map_err(|_| EFAULT)?;
    Ok(0)
}

fn socket_bind(arguments: [usize; 6]) -> SysResult {
    let (address, port) = read_sockaddr(arguments[1])?;
    crate::fd::bind(arguments[0] as i32, address, port).map(|_| 0)
}

fn socket_accept(arguments: [usize; 6], flags: u32) -> SysResult {
    let (fd, remote) = crate::fd::accept(arguments[0] as i32, flags)?;
    if arguments[1] != 0 {
        write_sockaddr(arguments[1], arguments[2], remote)?;
    }
    Ok(fd as usize)
}

fn socket_name(arguments: [usize; 6], local: bool) -> SysResult {
    let endpoint = if local {
        crate::fd::local_endpoint(arguments[0] as i32)?
    } else {
        crate::fd::remote_endpoint(arguments[0] as i32)?
    };
    write_sockaddr(arguments[1], arguments[2], endpoint)
}

fn socket_get_option(arguments: [usize; 6]) -> SysResult {
    crate::fd::socket_id(arguments[0] as i32)?;
    if arguments[3] != 0 {
        memory::write_user(arguments[3], &0i32).map_err(|_| EFAULT)?;
    }
    if arguments[4] != 0 {
        memory::write_user(arguments[4], &4u32).map_err(|_| EFAULT)?;
    }
    Ok(0)
}

fn exit(status: usize) -> ! {
    crate::println!("process: nginx exited with status {status}");
    crate::sbi::shutdown()
}
