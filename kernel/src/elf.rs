//! ELF64 loader for RISC-V user programs; builds the initial user stack
//! (argc/argv/envp/auxv) per the Linux riscv64 ABI.

use alloc::string::String;
use alloc::vec::Vec;

use crate::mm::paging;
use crate::mm::vma::{Mm, USER_STACK_TOP, PROT_READ, PROT_WRITE, PROT_EXEC};

pub const STACK_PAGES: usize = 256; // 1 MiB stack + guard

pub struct LoadResult {
    pub entry: usize,
    pub sp: usize,
    pub phdr: usize,
    pub phnum: usize,
    pub phent: usize,
}

fn rd16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn rd32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn rd64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        d[o], d[o + 1], d[o + 2], d[o + 3], d[o + 4], d[o + 5], d[o + 6], d[o + 7],
    ])
}

pub fn load_elf(
    mm: &mut Mm,
    data: &[u8],
    argv: &[String],
    envp: &[String],
    execfn: &str,
) -> Result<LoadResult, i32> {
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return Err(-8); // ENOEXEC
    }
    let class = data[4];
    if class != 2 {
        return Err(-8);
    }
    let e_type = rd16(data, 16);
    let e_machine = rd16(data, 18);
    if e_machine != 243 {
        return Err(-8);
    }
    let e_entry = rd64(data, 24) as usize;
    let e_phoff = rd64(data, 32) as usize;
    let e_phentsize = rd16(data, 54) as usize;
    let e_phnum = rd16(data, 56) as usize;

    let base = if e_type == 3 { 0x20000usize } else { 0usize }; // PIE support

    let mut phdr_addr = 0usize;
    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + 56 > data.len() {
            return Err(-8);
        }
        let p_type = rd32(data, off);
        let p_flags = rd32(data, off + 4);
        let p_offset = rd64(data, off + 8) as usize;
        let p_vaddr = rd64(data, off + 16) as usize;
        let p_filesz = rd64(data, off + 32) as usize;
        let p_memsz = rd64(data, off + 40) as usize;
        let _p_align = rd64(data, off + 48) as usize;
        crate::kprintln!("[elf] phdr[{}] type={} flags={:#x} vaddr={:#x} offset={:#x} filesz={:#x} memsz={:#x}", i, p_type, p_flags, p_vaddr, p_offset, p_filesz, p_memsz);
        match p_type {
            1 => {
                // PT_LOAD
                let start = base + p_vaddr;
                let end = base + p_vaddr + p_memsz;
                let mut prot = 0;
                if p_flags & 4 != 0 {
                    prot |= PROT_READ;
                }
                if p_flags & 2 != 0 {
                    prot |= PROT_WRITE;
                }
                if p_flags & 1 != 0 {
                    prot |= PROT_EXEC;
                }
                let page_start = start & !(paging::PAGE_SIZE - 1);
                let page_end = (end + paging::PAGE_SIZE - 1) & !(paging::PAGE_SIZE - 1);
                mm.map_file(page_start, page_end, prot, data, p_vaddr, p_offset, p_filesz);
            }
            6 => {
                // PT_PHDR
                phdr_addr = base + p_vaddr;
            }
            _ => {}
        }
    }
    // bss beyond file end is zeroed by map_file.

    // user stack (with an unmapped guard page below)
    let stack_top = USER_STACK_TOP;
    let stack_bottom = stack_top - STACK_PAGES * paging::PAGE_SIZE;
    mm.map_anon(stack_bottom + paging::PAGE_SIZE, stack_top, PROT_READ | PROT_WRITE);
    mm.stack_top = stack_top;

    // build the stack image (write through the new page table)
    let sp = build_stack(mm, argv, envp, execfn, stack_top, base + e_entry, phdr_addr, e_phnum, e_phentsize);

    Ok(LoadResult {
        entry: base + e_entry,
        sp,
        phdr: phdr_addr,
        phnum: e_phnum,
        phent: e_phentsize,
    })
}

fn push_u64(stack: &mut Vec<u8>, v: u64) {
    stack.extend_from_slice(&v.to_le_bytes());
}

fn build_stack(
    mm: &mut Mm,
    argv: &[String],
    envp: &[String],
    execfn: &str,
    stack_top: usize,
    entry: usize,
    phdr: usize,
    phnum: usize,
    phent: usize,
) -> usize {
    let mut strings: Vec<u8> = Vec::new();
    let mut argv_off: Vec<usize> = Vec::new();
    for a in argv {
        argv_off.push(strings.len());
        strings.extend_from_slice(a.as_bytes());
        strings.push(0);
    }
    let mut envp_off: Vec<usize> = Vec::new();
    for e in envp {
        envp_off.push(strings.len());
        strings.extend_from_slice(e.as_bytes());
        strings.push(0);
    }
    let execfn_off = strings.len();
    strings.extend_from_slice(execfn.as_bytes());
    strings.push(0);

    // random 16 bytes
    let random_off = strings.len();
    let mut seed = crate::timer::rdtime() as u64;
    for _ in 0..16 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        strings.push((seed >> 56) as u8);
    }

    let n_argv = argv.len();
    let n_envp = envp.len();

    // auxv entries (pairs)
    let mut auxv: Vec<(u64, u64)> = Vec::new();
    auxv.push((3, phdr as u64)); // AT_PHDR
    auxv.push((4, phent as u64)); // AT_PHENT
    auxv.push((5, phnum as u64)); // AT_PHNUM
    auxv.push((6, paging::PAGE_SIZE as u64)); // AT_PAGESZ
    auxv.push((7, 0)); // AT_BASE (no interpreter)
    auxv.push((8, 0)); // AT_FLAGS
    auxv.push((9, entry as u64)); // AT_ENTRY
    auxv.push((11, 0)); // AT_UID
    auxv.push((12, 0)); // AT_EUID
    auxv.push((13, 0)); // AT_GID
    auxv.push((14, 0)); // AT_EGID
    auxv.push((16, 0)); // AT_HWCAP
    auxv.push((17, 100)); // AT_CLKTCK
    auxv.push((15, 0)); // AT_PLATFORM (not provided)
    auxv.push((23, 0)); // AT_SECURE
    auxv.push((25, 0)); // AT_RANDOM (patched below)
    auxv.push((31, 0)); // AT_EXECFN (patched below)
    auxv.push((0, 0)); // AT_NULL

    // sizes
    let strings_len = strings.len();
    let auxv_len = auxv.len() * 16;
    let ptrs_len = 8 * (n_argv + 1 + n_envp + 1);
    let total = 8 + ptrs_len + auxv_len + strings_len;
    let sp = (stack_top - total) & !15;

    // string area base
    let strings_base = sp + 8 + ptrs_len + auxv_len;
    let random_addr = strings_base + random_off;
    let execfn_addr = strings_base + execfn_off;

    // patch auxv
    for e in auxv.iter_mut() {
        if e.0 == 25 {
            e.1 = random_addr as u64;
        }
        if e.0 == 31 {
            e.1 = execfn_addr as u64;
        }
    }

    let mut stack = Vec::with_capacity(total);
    // argc
    push_u64(&mut stack, n_argv as u64);
    // argv pointers
    for i in 0..n_argv {
        push_u64(&mut stack, (strings_base + argv_off[i]) as u64);
    }
    push_u64(&mut stack, 0);
    // envp pointers
    for i in 0..n_envp {
        push_u64(&mut stack, (strings_base + envp_off[i]) as u64);
    }
    push_u64(&mut stack, 0);
    // auxv
    for (k, v) in auxv {
        push_u64(&mut stack, k);
        push_u64(&mut stack, v);
    }
    // strings + random
    stack.extend_from_slice(&strings);
    debug_assert_eq!(stack.len(), total);
    // write to user memory through the new page table (satp not switched yet)
    let mut offset = 0usize;
    while offset < stack.len() {
        let va = sp + offset;
        let phys = mm.pt.translate(va).expect("stack page mapped");
        let n = core::cmp::min(paging::PAGE_SIZE - (va & 0xfff), stack.len() - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(stack.as_ptr().add(offset), phys as *mut u8, n);
        }
        offset += n;
    }
    sp
}
