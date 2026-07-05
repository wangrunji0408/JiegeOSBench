//! ELF64 loader with PT_INTERP (dynamic linker) support, and the SysV
//! argv/envp/auxv stack layout.
use crate::mm::paging::{PTE_R, PTE_U, PTE_W, PTE_X};
use crate::mm::{page_down, PAGE_SIZE};
use crate::task::{Task, ELF_BASE, INTERP_BASE, STACK_SIZE, STACK_TOP};
use alloc::string::String;
use alloc::vec::Vec;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct Ehdr {
    ident: [u8; 16],
    etype: u16,
    machine: u16,
    version: u32,
    entry: u64,
    phoff: u64,
    shoff: u64,
    flags: u32,
    ehsize: u16,
    phentsize: u16,
    phnum: u16,
    shentsize: u16,
    shnum: u16,
    shstrndx: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Phdr {
    ptype: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

struct LoadedElf {
    entry: usize,   // actual entry va
    phdr_va: usize, // va of program headers
    phnum: usize,
    max_va: usize, // highest mapped va (for brk placement)
    interp: Option<String>,
}

fn load_one(task: &mut Task, data: &[u8], base: usize) -> LoadedElf {
    assert_eq!(&data[..4], b"\x7fELF", "not an ELF");
    let eh: Ehdr = unsafe { core::ptr::read_unaligned(data.as_ptr() as *const Ehdr) };
    let load_base = if eh.etype == 2 { 0 } else { base }; // ET_EXEC vs ET_DYN
    let mut interp = None;
    let mut max_va = 0usize;
    let mut phdr_va = 0usize;

    for i in 0..eh.phnum as usize {
        let off = eh.phoff as usize + i * eh.phentsize as usize;
        let ph: Phdr = unsafe { core::ptr::read_unaligned(data.as_ptr().add(off) as *const Phdr) };
        match ph.ptype {
            PT_INTERP => {
                let s = &data[ph.offset as usize..(ph.offset + ph.filesz) as usize];
                let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
                interp = Some(String::from_utf8_lossy(&s[..end]).into_owned());
            }
            PT_LOAD => {
                let va = load_base + ph.vaddr as usize;
                // RWX for simplicity; ld.so will mprotect as needed
                let flags = PTE_U | PTE_R | PTE_W | PTE_X;
                task.map_range(va, ph.memsz as usize, flags);
                let src = &data[ph.offset as usize..(ph.offset + ph.filesz) as usize];
                task.write_user(va, src);
                // zero the bss tail within the last partially-used page is
                // unnecessary: fresh frames are zeroed, but if filesz lands
                // mid-page of a previously written page, zero explicitly.
                max_va = max_va.max(va + ph.memsz as usize);
                // where do program headers live in memory?
                let ph_file_start = eh.phoff as usize;
                let ph_file_end = ph_file_start + eh.phnum as usize * eh.phentsize as usize;
                if (ph.offset as usize) <= ph_file_start
                    && ph_file_end <= (ph.offset + ph.filesz) as usize
                {
                    phdr_va = va + (ph_file_start - ph.offset as usize);
                }
            }
            _ => {}
        }
    }
    if phdr_va == 0 {
        // program headers not covered by any PT_LOAD; copy them somewhere
        let va = load_base + page_down(max_va) + PAGE_SIZE;
        let sz = eh.phnum as usize * eh.phentsize as usize;
        task.map_range(va, sz, PTE_U | PTE_R);
        task.write_user(va, &data[eh.phoff as usize..eh.phoff as usize + sz]);
        phdr_va = va;
        max_va = max_va.max(va + sz);
    }
    LoadedElf {
        entry: load_base + eh.entry as usize,
        phdr_va,
        phnum: eh.phnum as usize,
        max_va,
        interp,
    }
}

pub struct ExecInfo {
    pub entry: usize,
    pub sp: usize,
}

/// Load `path` with args/env, set up the stack, return entry + sp.
pub fn exec(task: &mut Task, path: &str, argv: &[&str], envp: &[&str]) -> ExecInfo {
    let data = crate::fs::with_fs(|fs| fs.lookup_file(path)).expect("exec: no such file");
    let data = data.lock().clone();
    let main_elf = load_one(task, &data, ELF_BASE);

    let (entry, at_base) = if let Some(ref interp_path) = main_elf.interp {
        println!("[loader] interpreter: {}", interp_path);
        let idata = crate::fs::with_fs(|fs| fs.lookup_file(interp_path.as_str()))
            .expect("exec: interpreter not found");
        let idata = idata.lock().clone();
        let interp_elf = load_one(task, &idata, INTERP_BASE);
        (interp_elf.entry, INTERP_BASE)
    } else {
        (main_elf.entry, 0)
    };

    // brk right after the main image (leave a gap)
    task.brk_start = crate::mm::page_up(main_elf.max_va) + 16 * PAGE_SIZE;
    task.brk = task.brk_start;

    // stack
    task.map_range(STACK_TOP - STACK_SIZE, STACK_SIZE, PTE_U | PTE_R | PTE_W);

    // ---- build initial stack ----
    let mut sp = STACK_TOP;
    let mut push_bytes = |sp: &mut usize, b: &[u8]| -> usize {
        *sp -= b.len();
        task.write_user(*sp, b);
        *sp
    };
    // strings
    let mut argv_ptrs = Vec::new();
    for a in argv {
        let mut s = Vec::from(a.as_bytes());
        s.push(0);
        argv_ptrs.push(push_bytes(&mut sp, &s));
    }
    let mut env_ptrs = Vec::new();
    for e in envp {
        let mut s = Vec::from(e.as_bytes());
        s.push(0);
        env_ptrs.push(push_bytes(&mut sp, &s));
    }
    let execfn = argv_ptrs.first().copied().unwrap_or(0);
    // AT_RANDOM: 16 bytes
    let rand_bytes: [u8; 16] = [
        0x8a, 0x1f, 0x3c, 0x5e, 0x77, 0x02, 0xb9, 0xd4, 0x41, 0x6b, 0xe0, 0x9d, 0x2f, 0x58, 0xc3,
        0x66,
    ];
    let at_random = push_bytes(&mut sp, &rand_bytes);

    sp &= !0xf;

    let auxv: &[(usize, usize)] = &[
        (3, main_elf.phdr_va),         // AT_PHDR
        (4, 56),                       // AT_PHENT
        (5, main_elf.phnum),           // AT_PHNUM
        (6, PAGE_SIZE),                // AT_PAGESZ
        (7, at_base),                  // AT_BASE
        (8, 0),                        // AT_FLAGS
        (9, main_elf.entry),           // AT_ENTRY
        (11, 0),                       // AT_UID
        (12, 0),                       // AT_EUID
        (13, 0),                       // AT_GID
        (14, 0),                       // AT_EGID
        (16, 0x0000000000112d),        // AT_HWCAP (imafdc)
        (17, 100),                     // AT_CLKTCK
        (23, 0),                       // AT_SECURE
        (25, at_random),               // AT_RANDOM
        (31, execfn),                  // AT_EXECFN
        (0, 0),                        // AT_NULL
    ];

    // total words: argc + argv + NULL + env + NULL + auxv*2
    let words = 1 + argv_ptrs.len() + 1 + env_ptrs.len() + 1 + auxv.len() * 2;
    // keep sp 16-aligned after placing the table
    if words % 2 == 1 {
        sp -= 8;
    }
    sp -= words * 8;
    let mut w = sp;
    let mut push_word = |w: &mut usize, v: usize| {
        task.write_user(*w, &v.to_le_bytes());
        *w += 8;
    };
    push_word(&mut w, argv_ptrs.len()); // argc
    for p in &argv_ptrs {
        push_word(&mut w, *p);
    }
    push_word(&mut w, 0);
    for p in &env_ptrs {
        push_word(&mut w, *p);
    }
    push_word(&mut w, 0);
    for (k, v) in auxv {
        push_word(&mut w, *k);
        push_word(&mut w, *v);
    }

    println!(
        "[loader] entry={:#x} sp={:#x} phdr={:#x} brk={:#x}",
        entry, sp, main_elf.phdr_va, task.brk
    );
    ExecInfo { entry, sp }
}
