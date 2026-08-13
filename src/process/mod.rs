//! Process management: address spaces, loading, and the current process.

pub mod elf;

use crate::memory::frame;
use crate::memory::page_table::{self, PageTable};
use crate::memory::PAGE_SIZE;
use crate::sync::SpinLock;
use crate::trap::TrapContext;
use alloc::string::String;
use alloc::vec::Vec;

/// Kernel stack size per process (physical, identity-mapped).
pub const KSTACK_SIZE: usize = 16 * 1024;
/// Top of the user stack (just below the Sv39 low-half limit).
pub const USER_STACK_TOP: usize = 0x0000_003f_ffff_0000;
pub const USER_STACK_SIZE: usize = 8 * 1024 * 1024;

// auxv types
pub const AT_NULL: usize = 0;
pub const AT_IGNORE: usize = 1;
pub const AT_EXECFD: usize = 2;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_FLAGS: usize = 8;
pub const AT_ENTRY: usize = 9;
pub const AT_UID: usize = 11;
pub const AT_EUID: usize = 12;
pub const AT_GID: usize = 13;
pub const AT_EGID: usize = 14;
pub const AT_HWCAP: usize = 16;
pub const AT_CLKTCK: usize = 17;
pub const AT_SECURE: usize = 23;
pub const AT_RANDOM: usize = 25;
pub const AT_EXECFN: usize = 31;

static NEXT_PID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Static pool of kernel stacks (identity-mapped, so their addresses are
/// directly usable as kernel stack pointers). Kernel stacks must be contiguous,
/// so we reserve them statically rather than from the frame free list.
const MAX_PROCS: usize = 16;
#[repr(align(16))]
#[derive(Clone, Copy)]
struct KStack([u8; KSTACK_SIZE]);
static KSTACKS: [KStack; MAX_PROCS] = [KStack([0; KSTACK_SIZE]); MAX_PROCS];
static KSTACK_USED: SpinLock<u64> = SpinLock::new(0);

fn alloc_kstack() -> usize {
    let mut used = KSTACK_USED.lock();
    for i in 0..MAX_PROCS {
        if *used & (1 << i) == 0 {
            *used |= 1 << i;
            return &raw const KSTACKS[i] as usize + KSTACK_SIZE;
        }
    }
    panic!("out of kernel stacks");
}

#[allow(dead_code)]
fn free_kstack(top: usize) {
    let idx = (top - KSTACK_SIZE - &raw const KSTACKS[0] as usize) / KSTACK_SIZE;
    let mut used = KSTACK_USED.lock();
    *used &= !(1 << idx);
}

pub struct Process {
    pub pid: u32,
    pub page_table: PageTable,
    pub kstack_top: usize,
    pub trap_cx: usize,
    pub brk: usize,
    pub mmap_hint: usize,
    pub cwd: String,
    pub clear_child_tid: usize,
    pub exited: bool,
    pub fds: Vec<Option<crate::fs::FileDesc>>,
}

impl Process {
    fn next_pid() -> u32 {
        NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    }

    /// Create a new process from an ELF image (in-memory), with argv/envp.
    pub fn from_elf(elf: &[u8], argv: &[&str], envp: &[&str]) -> Process {
        let mut pt = page_table::kernel_page_table();
        let loaded = elf::load(&mut pt, elf).expect("failed to load ELF");

        let mut proc = Process {
            pid: Self::next_pid(),
            page_table: pt,
            kstack_top: 0,
            trap_cx: 0,
            brk: frame::align_up(loaded.max_va, PAGE_SIZE),
            mmap_hint: frame::align_up(loaded.max_va + 0x1000_0000, PAGE_SIZE),
            cwd: String::from("/"),
            clear_child_tid: 0,
            exited: false,
            fds: crate::fs::default_fds(),
        };

        // Set up kernel stack and trap context.
        proc.kstack_top = alloc_kstack();
        proc.trap_cx = proc.kstack_top - core::mem::size_of::<TrapContext>();

        // Build user stack.
        let sp = build_stack(&mut proc.page_table, argv, envp, &loaded);
        let cx = TrapContext::new_user(loaded.entry, sp);
        unsafe {
            *(proc.trap_cx as *mut TrapContext) = cx;
        }

        proc
    }

    pub fn trap_cx_ptr(&self) -> *mut TrapContext {
        self.trap_cx as *mut TrapContext
    }

    /// Make this process's address space active (set satp).
    pub fn activate(&self) {
        unsafe {
            let satp = self.page_table.satp();
            core::arch::asm!("csrw satp, {}", in(reg) satp);
            core::arch::asm!("sfence.vma");
        }
    }
}

pub static CURRENT: SpinLock<Option<Process>> = SpinLock::new(None);

pub fn current() -> &'static SpinLock<Option<Process>> {
    &CURRENT
}

/// Write bytes into the user address space via the page table (kernel mode,
/// SUM may be off), translating each page.
pub fn write_user(pt: &PageTable, va: usize, data: &[u8]) {
    let mut off = 0;
    while off < data.len() {
        let cur = va + off;
        let pa = pt.translate(cur).unwrap_or_else(|| panic!("write_user: unmapped va {:#x}", cur));
        let page_off = cur & 0xfff;
        let n = (PAGE_SIZE - page_off).min(data.len() - off);
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(off), pa as *mut u8, n);
        }
        off += n;
    }
}

/// Read bytes from the user address space via the page table.
pub fn read_user(pt: &PageTable, va: usize, buf: &mut [u8]) {
    let mut off = 0;
    while off < buf.len() {
        let cur = va + off;
        let pa = pt.translate(cur).unwrap_or_else(|| panic!("read_user: unmapped va {:#x}", cur));
        let page_off = cur & 0xfff;
        let n = (PAGE_SIZE - page_off).min(buf.len() - off);
        unsafe {
            core::ptr::copy_nonoverlapping(pa as *const u8, buf.as_mut_ptr().add(off), n);
        }
        off += n;
    }
}

/// Build the initial user stack; returns the new stack pointer.
///
/// Layout (from low to high address): strings, random bytes, auxv, envp
/// pointers, argv pointers, argc. `sp` points at argc.
fn build_stack(pt: &mut PageTable, argv: &[&str], envp: &[&str], loaded: &elf::LoadedElf) -> usize {
    // Map the stack region.
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
    let mut va = stack_bottom;
    while va < USER_STACK_TOP {
        let f = frame::alloc().expect("out of frames for user stack");
        pt.map(va, f.0, page_table::USER_RW);
        va += PAGE_SIZE;
    }

    // Assemble string blob.
    let mut strings: Vec<u8> = Vec::new();
    let mut argv_off: Vec<usize> = Vec::new();
    for s in argv {
        argv_off.push(strings.len());
        strings.extend_from_slice(s.as_bytes());
        strings.push(0);
    }
    let mut envp_off: Vec<usize> = Vec::new();
    for s in envp {
        envp_off.push(strings.len());
        strings.extend_from_slice(s.as_bytes());
        strings.push(0);
    }
    while strings.len() % 16 != 0 {
        strings.push(0);
    }
    let strings_size = strings.len();

    // Random bytes for AT_RANDOM (16 bytes).
    let random: [u8; 16] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00];

    // auxv entries
    let mut auxv: Vec<(usize, usize)> = vec![
        (AT_PHDR, loaded.phdr),
        (AT_PHENT, loaded.phentsize),
        (AT_PHNUM, loaded.phnum),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, 0),
        (AT_ENTRY, loaded.entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_CLKTCK, 100),
        (AT_HWCAP, 0),
        (AT_SECURE, 0),
        (AT_RANDOM, 0), // patched below
        (AT_NULL, 0),
    ];
    let auxv_size = auxv.len() * 16;

    let argc_size = 8usize;
    let argv_size = (argv.len() + 1) * 8;
    let envp_size = (envp.len() + 1) * 8;
    let random_size = 16usize;

    let total = strings_size + random_size + auxv_size + envp_size + argv_size + argc_size;
    let total_aligned = frame::align_up(total, 16);
    let sp = USER_STACK_TOP - total_aligned;

    // Absolute addresses.
    let strings_addr = sp + pad;
    let random_addr = strings_addr + strings_size;
    let auxv_addr = random_addr + random_size;
    let envp_addr = auxv_addr + auxv_size;
    let argv_addr = envp_addr + envp_size;
    let argc_addr = argv_addr + argv_size;

    // Patch AT_RANDOM to point at the random bytes.
    for e in auxv.iter_mut() {
        if e.0 == AT_RANDOM {
            e.1 = random_addr;
        }
    }

    // Write strings.
    write_user(pt, strings_addr, &strings);
    // Write random bytes.
    write_user(pt, random_addr, &random);
    // Write auxv.
    {
        let mut blob = Vec::with_capacity(auxv_size);
        for &(t, v) in &auxv {
            blob.extend_from_slice(&(t as u64).to_le_bytes());
            blob.extend_from_slice(&(v as u64).to_le_bytes());
        }
        write_user(pt, auxv_addr, &blob);
    }
    // Write envp pointers.
    {
        let mut blob = Vec::with_capacity(envp_size);
        for &o in &envp_off {
            blob.extend_from_slice(&((strings_addr + o) as u64).to_le_bytes());
        }
        blob.extend_from_slice(&0u64.to_le_bytes());
        write_user(pt, envp_addr, &blob);
    }
    // Write argv pointers.
    {
        let mut blob = Vec::with_capacity(argv_size);
        for &o in &argv_off {
            blob.extend_from_slice(&((strings_addr + o) as u64).to_le_bytes());
        }
        blob.extend_from_slice(&0u64.to_le_bytes());
        write_user(pt, argv_addr, &blob);
    }
    // Write argc.
    write_user(pt, argc_addr, &(argv.len() as u64).to_le_bytes());

    sp
}
