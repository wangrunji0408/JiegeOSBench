use crate::config::*;
use crate::page_table::*;
use crate::task::Task;
use alloc::vec::Vec;

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(b[off..off + 2].try_into().unwrap())
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

const PT_LOAD: u32 = 1;

// auxv keys
const AT_NULL: usize = 0;
const AT_PHDR: usize = 3;
const AT_PHENT: usize = 4;
const AT_PHNUM: usize = 5;
const AT_PAGESZ: usize = 6;
const AT_BASE: usize = 7;
const AT_FLAGS: usize = 8;
const AT_ENTRY: usize = 9;
const AT_UID: usize = 11;
const AT_EUID: usize = 12;
const AT_GID: usize = 13;
const AT_EGID: usize = 14;
const AT_HWCAP: usize = 16;
const AT_CLKTCK: usize = 17;
const AT_SECURE: usize = 23;
const AT_RANDOM: usize = 25;
const AT_EXECFN: usize = 31;

const PERM: usize = PTE_R | PTE_W | PTE_X;

unsafe fn wr_bytes(dst: usize, src: &[u8]) {
    core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len());
}
unsafe fn wr_usize(dst: usize, v: usize) {
    (dst as *mut usize).write(v);
}

/// Load a static (non-PIE) ELF into the current task's address space and build
/// the initial user stack. Returns (entry, user_sp). The task's page table must
/// already be active (satp switched) so user VAs are writable here.
pub fn load(task: &mut Task, elf: &[u8], argv: &[&str], envp: &[&str]) -> (usize, usize) {
    assert_eq!(&elf[0..4], b"\x7fELF", "not an ELF");
    let entry = rd_u64(elf, 24) as usize;
    let phoff = rd_u64(elf, 32) as usize;
    let phentsize = rd_u16(elf, 54) as usize;
    let phnum = rd_u16(elf, 56) as usize;

    let mut brk_end = 0usize;
    let mut at_phdr = 0usize;

    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let p_type = rd_u32(elf, ph);
        if p_type != PT_LOAD {
            continue;
        }
        let p_offset = rd_u64(elf, ph + 8) as usize;
        let p_vaddr = rd_u64(elf, ph + 16) as usize;
        let p_filesz = rd_u64(elf, ph + 32) as usize;
        let p_memsz = rd_u64(elf, ph + 40) as usize;

        task.map_user(p_vaddr, p_memsz, PERM);
        unsafe {
            wr_bytes(p_vaddr, &elf[p_offset..p_offset + p_filesz]);
            // bss (memsz > filesz) is already zeroed by the frame allocator.
        }
        if p_offset <= phoff && phoff < p_offset + p_filesz {
            at_phdr = p_vaddr + (phoff - p_offset);
        }
        let seg_end = p_vaddr + p_memsz;
        if seg_end > brk_end {
            brk_end = seg_end;
        }
    }
    task.brk = (brk_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Build the user stack.
    let stack_top = USER_STACK_TOP;
    task.map_user(stack_top - USER_STACK_SIZE, USER_STACK_SIZE, PERM);
    let mut sp = stack_top;

    // Push strings (argv, envp), execfn, random.
    let mut push_str = |sp: &mut usize, s: &str| -> usize {
        let bytes = s.as_bytes();
        *sp -= bytes.len() + 1;
        unsafe {
            wr_bytes(*sp, bytes);
            (*sp + bytes.len() as usize as usize as usize) as usize;
            *(( *sp + bytes.len()) as *mut u8) = 0;
        }
        *sp
    };

    let mut argv_ptrs = Vec::new();
    for a in argv {
        argv_ptrs.push(push_str(&mut sp, a));
    }
    let mut envp_ptrs = Vec::new();
    for e in envp {
        envp_ptrs.push(push_str(&mut sp, e));
    }
    let execfn = push_str(&mut sp, argv.get(0).copied().unwrap_or("prog"));

    // 16 random bytes.
    sp -= 16;
    let random_ptr = sp;
    unsafe {
        for k in 0..16 {
            *((random_ptr + k) as *mut u8) = (0x5a ^ (k as u8).wrapping_mul(31)) as u8;
        }
    }

    // auxv entries.
    let aux: [(usize, usize); 16] = [
        (AT_PHDR, at_phdr),
        (AT_PHENT, phentsize),
        (AT_PHNUM, phnum),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, 0),
        (AT_FLAGS, 0),
        (AT_ENTRY, entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_HWCAP, 0),
        (AT_CLKTCK, 100),
        (AT_SECURE, 0),
        (AT_RANDOM, random_ptr),
        (AT_EXECFN, execfn),
    ];

    // Compute size of the info block so argc lands 16-byte aligned.
    let argc = argv.len();
    let nwords = 1                       // argc
        + (argc + 1)                     // argv + null
        + (envp.len() + 1)               // envp + null
        + (aux.len() + 1) * 2;           // auxv pairs + AT_NULL pair
    let block = nwords * 8;
    sp = (sp - block) & !15;
    let base = sp;

    unsafe {
        let mut p = base;
        wr_usize(p, argc);
        p += 8;
        for &a in &argv_ptrs {
            wr_usize(p, a);
            p += 8;
        }
        wr_usize(p, 0);
        p += 8;
        for &e in &envp_ptrs {
            wr_usize(p, e);
            p += 8;
        }
        wr_usize(p, 0);
        p += 8;
        for &(k, v) in &aux {
            wr_usize(p, k);
            p += 8;
            wr_usize(p, v);
            p += 8;
        }
        wr_usize(p, AT_NULL);
        p += 8;
        wr_usize(p, 0);
    }

    (entry, base)
}
