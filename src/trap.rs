use core::{
    arch::{asm, global_asm},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::memory;

pub const USER_STACK_TOP: usize = 0x0000_003f_ffff_f000;

#[repr(C)]
pub struct TrapFrame {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
}

impl TrapFrame {
    const fn zero() -> Self {
        Self {
            x: [0; 32],
            sstatus: 0,
            sepc: 0,
        }
    }
}

#[repr(C, align(16))]
struct TrapArea {
    stack: [u8; 64 * 1024],
    frame: TrapFrame,
}

static mut TRAP_AREA: TrapArea = TrapArea {
    stack: [0; 64 * 1024],
    frame: TrapFrame::zero(),
};

global_asm!(
    r#"
    .align 4
    .globl __trap_vector
__trap_vector:
    csrrw sp, sscratch, sp
    sd ra,   8(sp)
    sd gp,  24(sp)
    sd tp,  32(sp)
    sd t0,  40(sp)
    sd t1,  48(sp)
    sd t2,  56(sp)
    sd s0,  64(sp)
    sd s1,  72(sp)
    sd a0,  80(sp)
    sd a1,  88(sp)
    sd a2,  96(sp)
    sd a3, 104(sp)
    sd a4, 112(sp)
    sd a5, 120(sp)
    sd a6, 128(sp)
    sd a7, 136(sp)
    sd s2, 144(sp)
    sd s3, 152(sp)
    sd s4, 160(sp)
    sd s5, 168(sp)
    sd s6, 176(sp)
    sd s7, 184(sp)
    sd s8, 192(sp)
    sd s9, 200(sp)
    sd s10,208(sp)
    sd s11,216(sp)
    sd t3, 224(sp)
    sd t4, 232(sp)
    sd t5, 240(sp)
    sd t6, 248(sp)
    csrr t0, sscratch
    sd t0,  16(sp)
    csrr t0, sstatus
    sd t0, 256(sp)
    csrr t0, sepc
    sd t0, 264(sp)
    mv a0, sp
    call rust_trap_handler

    .globl __restore_user
__restore_user:
    ld t0, 256(sp)
    csrw sstatus, t0
    ld t0, 264(sp)
    csrw sepc, t0
    csrw sscratch, sp
    ld ra,   8(sp)
    ld gp,  24(sp)
    ld tp,  32(sp)
    ld t0,  40(sp)
    ld t1,  48(sp)
    ld t2,  56(sp)
    ld s0,  64(sp)
    ld s1,  72(sp)
    ld a0,  80(sp)
    ld a1,  88(sp)
    ld a2,  96(sp)
    ld a3, 104(sp)
    ld a4, 112(sp)
    ld a5, 120(sp)
    ld a6, 128(sp)
    ld a7, 136(sp)
    ld s2, 144(sp)
    ld s3, 152(sp)
    ld s4, 160(sp)
    ld s5, 168(sp)
    ld s6, 176(sp)
    ld s7, 184(sp)
    ld s8, 192(sp)
    ld s9, 200(sp)
    ld s10,208(sp)
    ld s11,216(sp)
    ld t3, 224(sp)
    ld t4, 232(sp)
    ld t5, 240(sp)
    ld t6, 248(sp)
    ld sp,  16(sp)
    sret

    .globl __enter_user
__enter_user:
    csrw satp, a1
    sfence.vma zero, zero
    mv sp, a0
    j __restore_user
"#
);

unsafe extern "C" {
    fn __trap_vector();
    fn __enter_user(frame: *mut TrapFrame, satp: usize) -> !;
}

pub fn init() {
    unsafe { asm!("csrw stvec, {}", in(reg) __trap_vector as *const () as usize) };
}

pub fn enter(entry: usize, stack: usize, satp: usize) -> ! {
    let frame = unsafe { &raw mut TRAP_AREA.frame };
    unsafe {
        (*frame).x = [0; 32];
        (*frame).x[2] = stack;
        (*frame).sepc = entry;
        // User mode (SPP=0), interrupts enabled after sret (SPIE=1), FP state initial.
        (*frame).sstatus = (1 << 5) | (1 << 13);
        __enter_user(frame, satp)
    }
}

#[unsafe(no_mangle)]
extern "C" fn rust_trap_handler(frame: &mut TrapFrame) {
    let scause: usize;
    let stval: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) scause);
        asm!("csrr {}, stval", out(reg) stval);
    }
    match scause {
        8 => {
            frame.sepc += 4;
            let result = syscall(frame);
            frame.x[10] = result as usize;
        }
        _ => panic!(
            "user trap: scause={:#x} stval={:#x} sepc={:#x}",
            scause, stval, frame.sepc
        ),
    }
}

fn syscall(frame: &TrapFrame) -> isize {
    match frame.x[17] {
        17 => -2, // getcwd: no directory object yet
        19 => NEXT_AUX_FD.fetch_add(1, Ordering::Relaxed) as isize,
        20 => 100, // epoll_create1
        21 => crate::network::epoll_ctl(frame.x[12], frame.x[13]),
        22 => crate::network::epoll_wait(frame.x[11]),
        25 => 0, // fcntl
        29 => {
            if frame.x[10] >= 100 {
                0
            } else {
                -25
            }
        } // ioctl
        34 => 0, // mkdirat (initramfs directories are virtual)
        48 => 0, // faccessat
        56 => sys_openat(frame.x[11], frame.x[12]),
        57 => {
            if frame.x[10] == 102 {
                crate::network::close_connection()
            } else if frame.x[10] >= 100 {
                0
            } else {
                crate::fs::close(frame.x[10])
            }
        }
        62 => crate::fs::seek(frame.x[10], frame.x[11] as isize, frame.x[12]),
        63 => {
            if frame.x[10] == 102 {
                crate::network::receive(frame.x[11], frame.x[12])
            } else {
                crate::fs::read(frame.x[10], frame.x[11], frame.x[12])
            }
        }
        64 => sys_write(frame.x[10], frame.x[11], frame.x[12]),
        66 => sys_writev(frame.x[10], frame.x[11], frame.x[12]),
        67 => crate::fs::pread(frame.x[10], frame.x[11], frame.x[12], frame.x[13]),
        68 => crate::fs::write_sink(frame.x[10], frame.x[12]), // pwrite64
        71 => sys_sendfile(frame.x[10], frame.x[11], frame.x[13]),
        79 => sys_newfstatat(frame.x[11], frame.x[12]),
        80 => sys_fstat(frame.x[10], frame.x[11]),
        93 | 94 => {
            crate::println!("process exited with status {}", frame.x[10] as isize);
            crate::sbi::shutdown(frame.x[10] != 0)
        }
        96 | 99 | 100 => 0, // set_tid_address, robust list, get_robust_list
        113 => sys_clock_gettime(frame.x[11]),
        123 => sys_sched_getaffinity(frame.x[11], frame.x[12]),
        124 => 0,             // sched_yield
        129 | 134 | 135 => 0, // kill/sigaction/sigprocmask
        153 => 0,             // times
        160 => sys_uname(frame.x[10]),
        172 => 1,                      // getpid
        173 => 1,                      // getppid
        174 | 175 | 176 | 177 => 1000, // uid/euid/gid/egid
        178 => 1,                      // gettid
        198 => 101,                    // socket; network backend owns this descriptor
        199 => sys_socketpair(frame.x[13]),
        200 | 201 | 208 | 209 => 0, // bind/listen/socket options
        204 => crate::network::socket_name(frame.x[11], frame.x[12], false),
        205 => crate::network::socket_name(frame.x[11], frame.x[12], true),
        206 => crate::network::send(frame.x[11], frame.x[12]),
        207 => crate::network::receive(frame.x[11], frame.x[12]),
        214 => sys_brk(frame.x[10]),
        215 => 0, // munmap (address space is reclaimed on process exit)
        222 => sys_mmap(frame),
        226 => sys_mprotect(frame.x[10], frame.x[11], frame.x[12]),
        261 => sys_prlimit(frame.x[13]),
        278 => sys_getrandom(frame.x[10], frame.x[11]),
        242 => crate::network::accept(frame.x[11], frame.x[12]),
        number => {
            crate::println!("unsupported syscall {}", number);
            -38 // ENOSYS
        }
    }
}

fn sys_openat(path_address: usize, flags: usize) -> isize {
    let mut path = [0u8; 512];
    let length = match crate::fs::read_path_from_user(path_address, &mut path) {
        Ok(length) => length,
        Err(error) => return error,
    };
    let mut result = crate::fs::open(&path[..length]);
    if result == -2 && flags & 0x40 != 0 {
        result = crate::fs::create_sink();
    }
    if result < 0 {
        crate::println!(
            "openat missing: {}",
            core::str::from_utf8(&path[..length]).unwrap_or("?")
        );
    }
    result
}

fn sys_writev(fd: usize, vectors: usize, count: usize) -> isize {
    let mut total = 0isize;
    for index in 0..count {
        let Some(base) = memory::read_user_usize(vectors + index * 16) else {
            return -14;
        };
        let Some(length) = memory::read_user_usize(vectors + index * 16 + 8) else {
            return -14;
        };
        let result = sys_write(fd, base, length);
        if result < 0 {
            return result;
        }
        total += result;
    }
    total
}

fn write_u32(address: usize, value: u32) -> bool {
    value
        .to_le_bytes()
        .iter()
        .enumerate()
        .all(|(i, byte)| memory::write_user_byte(address + i, *byte))
}

fn write_stat(address: usize, size: usize, inode: usize) -> isize {
    if !memory::zero_user(address, 128) {
        return -14;
    }
    if !memory::write_user_usize(address, 1) {
        return -14;
    }
    if !memory::write_user_usize(address + 8, inode) {
        return -14;
    }
    if !write_u32(address + 16, 0o100_555) {
        return -14;
    }
    if !memory::write_user_usize(address + 48, size) {
        return -14;
    }
    if !memory::write_user_usize(address + 56, 4096) {
        return -14;
    }
    if !memory::write_user_usize(address + 64, size.div_ceil(512)) {
        return -14;
    }
    0
}

fn sys_fstat(fd: usize, stat: usize) -> isize {
    if fd <= 2 {
        return write_stat(stat, 0, fd + 1);
    }
    let Some((size, inode)) = crate::fs::metadata(fd) else {
        return -9;
    };
    write_stat(stat, size, inode)
}

fn sys_newfstatat(path_address: usize, stat: usize) -> isize {
    let mut path = [0u8; 512];
    let length = match crate::fs::read_path_from_user(path_address, &mut path) {
        Ok(length) => length,
        Err(error) => return error,
    };
    let Some((size, inode)) = crate::fs::path_metadata(&path[..length]) else {
        return -2;
    };
    write_stat(stat, size, inode)
}

static PROGRAM_BREAK: AtomicUsize = AtomicUsize::new(0x2_0000_0000);

fn sys_brk(request: usize) -> isize {
    let old = PROGRAM_BREAK.load(Ordering::SeqCst);
    if request == 0 {
        return old as isize;
    }
    if request > old {
        memory::active_page_table().map_user_memory(
            old,
            request - old,
            memory::PTE_R | memory::PTE_W,
        );
    }
    PROGRAM_BREAK.store(request, Ordering::SeqCst);
    request as isize
}

fn protection_flags(protection: usize) -> usize {
    let mut flags = 0;
    if protection & 1 != 0 {
        flags |= memory::PTE_R;
    }
    if protection & 2 != 0 {
        flags |= memory::PTE_W;
    }
    if protection & 4 != 0 {
        flags |= memory::PTE_X;
    }
    flags
}

fn sys_mmap(frame: &TrapFrame) -> isize {
    let requested = frame.x[10];
    let length = frame.x[11];
    let protection = frame.x[12];
    let fd = frame.x[14];
    let offset = frame.x[15];
    let fixed = frame.x[13] & 0x10 != 0;
    if length == 0 {
        return -22;
    }
    let address = if requested == 0 {
        memory::allocate_mmap_address(length)
    } else {
        requested
    };
    // A PROT_NONE mapping reserves an address range. Fixed segment mappings fill it later.
    if protection == 0 {
        return address as isize;
    }
    let table = memory::active_page_table();
    if fixed {
        table.replace_user_memory(address, length, protection_flags(protection));
    } else {
        table.map_user_memory(address, length, protection_flags(protection));
    }
    if fd != usize::MAX {
        let start = address & !(memory::PAGE_SIZE - 1);
        let end = (address + length + memory::PAGE_SIZE - 1) & !(memory::PAGE_SIZE - 1);
        let mut page = start;
        while page < end {
            let physical = table.translate(page).unwrap() & !(memory::PAGE_SIZE - 1);
            let file_offset = offset + page.saturating_sub(start);
            crate::fs::pread_to_physical(fd, file_offset, physical, memory::PAGE_SIZE);
            page += memory::PAGE_SIZE;
        }
    }
    address as isize
}

fn sys_mprotect(address: usize, length: usize, protection: usize) -> isize {
    if protection != 0 {
        memory::active_page_table().map_user_memory(address, length, protection_flags(protection));
    }
    0
}

fn sys_clock_gettime(output: usize) -> isize {
    if !memory::write_user_usize(output, 1_783_700_000) {
        return -14;
    }
    if !memory::write_user_usize(output + 8, 0) {
        return -14;
    }
    0
}

fn sys_uname(output: usize) -> isize {
    if !memory::zero_user(output, 65 * 6) {
        return -14;
    }
    for (field, text) in [
        b"JiegeOS\0" as &[u8],
        b"jiege\0",
        b"0.1\0",
        b"#1\0",
        b"riscv64\0",
    ]
    .iter()
    .enumerate()
    {
        for (index, byte) in text.iter().enumerate() {
            if !memory::write_user_byte(output + field * 65 + index, *byte) {
                return -14;
            }
        }
    }
    0
}

fn sys_prlimit(output: usize) -> isize {
    if output == 0 {
        return 0;
    }
    if !memory::write_user_usize(output, 1024) {
        return -14;
    }
    if !memory::write_user_usize(output + 8, 1024) {
        return -14;
    }
    0
}

fn sys_getrandom(output: usize, length: usize) -> isize {
    for index in 0..length {
        if !memory::write_user_byte(output + index, (index as u8).wrapping_mul(73) ^ 0xa5) {
            return -14;
        }
    }
    length as isize
}

fn sys_sched_getaffinity(output: usize, length: usize) -> isize {
    if length == 0 {
        return -22;
    }
    if !memory::zero_user(output, length) || !memory::write_user_byte(output, 1) {
        return -14;
    }
    length.min(8) as isize
}

static NEXT_AUX_FD: AtomicUsize = AtomicUsize::new(110);

fn sys_socketpair(output: usize) -> isize {
    let first = NEXT_AUX_FD.fetch_add(2, Ordering::Relaxed);
    if !write_u32(output, first as u32) || !write_u32(output + 4, (first + 1) as u32) {
        return -14;
    }
    0
}

fn sys_write(fd: usize, buffer: usize, length: usize) -> isize {
    if fd == 102 {
        return crate::network::send(buffer, length);
    }
    if fd >= 3 {
        return crate::fs::write_sink(fd, length);
    }
    if fd != 1 && fd != 2 {
        return -9; // EBADF
    }
    for offset in 0..length {
        let Some(byte) = memory::read_user_byte(buffer + offset) else {
            return -14; // EFAULT
        };
        crate::console::put_byte(byte);
    }
    length as isize
}

fn sys_sendfile(output_fd: usize, input_fd: usize, count: usize) -> isize {
    if output_fd != 102 {
        return -9;
    }
    let mut total = 0usize;
    let mut buffer = [0u8; 1400];
    while total < count {
        let wanted = (count - total).min(buffer.len());
        let read = crate::fs::read_kernel(input_fd, &mut buffer[..wanted]);
        if read < 0 {
            return read;
        }
        if read == 0 {
            break;
        }
        crate::network::send_bytes(&buffer[..read as usize]);
        total += read as usize;
    }
    total as isize
}
