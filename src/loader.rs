//! ELF loading, including dynamic executables.
//!
//! nginx is a PIE that needs `/lib/ld-musl-riscv64.so.1`, so we load both the
//! executable and its interpreter, then enter the interpreter with an auxiliary
//! vector describing the executable. The interpreter does the relocation work.

use crate::fs::{self, File, InodeRef, OpenFlags};
use crate::mm::{self, AddrSpace, Backing, Prot, PAGE_SIZE};
use crate::trap::TrapContext;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

// ELF constants.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_RISCV: u16 = 243;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_GNU_STACK: u32 = 0x6474_e551;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// The ELF64 header.
#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Header {
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

/// An ELF64 program header.
#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    ptype: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

/// Auxiliary vector types.
pub mod auxv {
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
    pub const AT_NOTELF: usize = 10;
    pub const AT_UID: usize = 11;
    pub const AT_EUID: usize = 12;
    pub const AT_GID: usize = 13;
    pub const AT_EGID: usize = 14;
    pub const AT_PLATFORM: usize = 15;
    pub const AT_HWCAP: usize = 16;
    pub const AT_CLKTCK: usize = 17;
    pub const AT_SECURE: usize = 23;
    pub const AT_BASE_PLATFORM: usize = 24;
    pub const AT_RANDOM: usize = 25;
    pub const AT_HWCAP2: usize = 26;
    pub const AT_EXECFN: usize = 31;
    pub const AT_SYSINFO: usize = 32;
    pub const AT_SYSINFO_EHDR: usize = 33;
    pub const AT_MINSIGSTKSZ: usize = 51;
}

/// The result of loading an ELF image.
pub struct LoadedImage {
    /// Where control should start (the interpreter's entry for dynamic
    /// executables, else the executable's own).
    pub entry: usize,
    /// The executable's own entry point.
    pub exec_entry: usize,
    /// Load bias applied to the executable.
    pub exec_base: usize,
    /// Where the interpreter was loaded, or 0 if static.
    pub interp_base: usize,
    /// User address of the program headers.
    pub phdr_addr: usize,
    pub phentsize: usize,
    pub phnum: usize,
    /// Highest address used by the executable, where the heap begins.
    pub brk: usize,
}

/// Read and validate an ELF header from a file.
fn read_header(file: &Arc<File>) -> fs::Result<Elf64Header> {
    let mut buf = [0u8; core::mem::size_of::<Elf64Header>()];
    let n = file.read_at(0, &mut buf)?;
    if n < buf.len() {
        crate::bail!(ENOEXEC);
    }
    let header: Elf64Header = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const _) };
    if header.ident[0..4] != ELF_MAGIC {
        crate::bail!(ENOEXEC);
    }
    if header.ident[4] != ELFCLASS64 || header.ident[5] != ELFDATA2LSB {
        crate::bail!(ENOEXEC);
    }
    if header.machine != EM_RISCV {
        crate::warn!("ELF machine {} is not riscv64", header.machine);
        crate::bail!(ENOEXEC);
    }
    if header.etype != ET_EXEC && header.etype != ET_DYN {
        crate::bail!(ENOEXEC);
    }
    if header.phnum == 0 || header.phnum > 128 {
        crate::bail!(ENOEXEC);
    }
    Ok(header)
}

/// Read the program headers.
fn read_phdrs(file: &Arc<File>, header: &Elf64Header) -> fs::Result<Vec<Elf64Phdr>> {
    let phentsize = header.phentsize as usize;
    if phentsize < core::mem::size_of::<Elf64Phdr>() {
        crate::bail!(ENOEXEC);
    }
    let mut phdrs = Vec::with_capacity(header.phnum as usize);
    let mut buf = alloc::vec![0u8; phentsize];
    for i in 0..header.phnum as usize {
        let off = header.phoff as usize + i * phentsize;
        let n = file.read_at(off, &mut buf)?;
        if n < core::mem::size_of::<Elf64Phdr>() {
            crate::bail!(ENOEXEC);
        }
        phdrs.push(unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Elf64Phdr) });
    }
    Ok(phdrs)
}

/// Translate ELF segment flags to VMA protection.
fn seg_prot(flags: u32) -> Prot {
    let mut prot = Prot::empty();
    if flags & PF_R != 0 {
        prot |= Prot::READ;
    }
    if flags & PF_W != 0 {
        prot |= Prot::WRITE;
    }
    if flags & PF_X != 0 {
        prot |= Prot::EXEC;
    }
    prot
}

/// Map the PT_LOAD segments of an ELF image at `bias`.
///
/// Returns the highest virtual address used.
fn map_segments(
    aspace: &mut AddrSpace,
    file: &Arc<File>,
    phdrs: &[Elf64Phdr],
    bias: usize,
    name: &'static str,
) -> fs::Result<usize> {
    let mut max_end = 0usize;

    for ph in phdrs.iter().filter(|p| p.ptype == PT_LOAD) {
        if ph.memsz == 0 {
            continue;
        }
        let vaddr = bias + ph.vaddr as usize;
        let file_end = vaddr + ph.filesz as usize;
        let mem_end = vaddr + ph.memsz as usize;
        let prot = seg_prot(ph.flags);

        // The file-backed part. Segments are not page aligned in general: the
        // ELF spec only guarantees `vaddr % align == offset % align`, so the
        // first page of a segment may need bytes from before `ph.offset`.
        let map_start = mm::page_down(vaddr);
        let page_offset = vaddr - map_start;
        let file_offset = ph.offset as usize - page_offset;

        if ph.filesz > 0 {
            aspace.map_region(
                map_start,
                mm::page_up(file_end),
                prot,
                Backing::File {
                    file: file.clone(),
                    offset: file_offset,
                },
                false,
                name,
            );
        }

        // The zero-filled tail (.bss). If it starts mid-page, that page is
        // already file-backed and the fault handler zeroes the rest, since it
        // reads at most `filesz` bytes into a freshly zeroed frame. Only whole
        // pages beyond need an anonymous mapping.
        let bss_page_start = mm::page_up(file_end);
        if mem_end > bss_page_start {
            aspace.map_region(
                bss_page_start,
                mm::page_up(mem_end),
                prot,
                Backing::Anon,
                false,
                name,
            );
        } else if ph.filesz == 0 {
            // Entirely anonymous segment.
            aspace.map_region(
                map_start,
                mm::page_up(mem_end),
                prot,
                Backing::Anon,
                false,
                name,
            );
        }

        max_end = max_end.max(mem_end);
    }

    if max_end == 0 {
        crate::bail!(ENOEXEC);
    }
    Ok(max_end)
}

/// Load an executable (and its interpreter) into a fresh address space.
pub fn load_elf(aspace: &mut AddrSpace, path: &str) -> fs::Result<LoadedImage> {
    let inode = fs::path::resolve(path, true)?;
    if inode.kind() != fs::InodeKind::File {
        crate::bail!(EACCES);
    }
    load_elf_inode(aspace, inode, path)
}

pub fn load_elf_inode(
    aspace: &mut AddrSpace,
    inode: InodeRef,
    path: &str,
) -> fs::Result<LoadedImage> {
    let file = Arc::new(File::with_path(inode, OpenFlags::RDONLY, path));
    let header = read_header(&file)?;
    let phdrs = read_phdrs(&file, &header)?;

    // A PIE (ET_DYN) needs a load bias; ET_EXEC is loaded at its own addresses.
    // Our user address space starts above 4 GiB, so an ET_EXEC linked at a low
    // address cannot be honored — but every modern riscv64 binary is a PIE.
    let exec_base = if header.etype == ET_DYN {
        mm::USER_ELF_BASE
    } else {
        let lowest = phdrs
            .iter()
            .filter(|p| p.ptype == PT_LOAD)
            .map(|p| p.vaddr as usize)
            .min()
            .unwrap_or(0);
        if !mm::is_user_addr(lowest) {
            crate::warn!(
                "ET_EXEC binary {} wants to load at {:#x}, below our user base {:#x}",
                path,
                lowest,
                mm::USER_BASE
            );
            crate::bail!(ENOEXEC);
        }
        0
    };

    let max_end = map_segments(aspace, &file, &phdrs, exec_base, "exe")?;

    // The program headers must be visible to the interpreter. They live inside
    // the first PT_LOAD segment in every real binary; find the segment that
    // covers `phoff` and compute the user address from it.
    let phdr_addr = phdrs
        .iter()
        .filter(|p| p.ptype == PT_LOAD)
        .find(|p| {
            let off = header.phoff;
            off >= p.offset && off + (header.phnum as u64 * header.phentsize as u64) <= p.offset + p.filesz
        })
        .map(|p| exec_base + (p.vaddr + (header.phoff - p.offset)) as usize)
        // Some binaries have an explicit PT_PHDR; prefer it if the search failed.
        .or_else(|| {
            phdrs
                .iter()
                .find(|p| p.ptype == PT_PHDR)
                .map(|p| exec_base + p.vaddr as usize)
        })
        .unwrap_or(0);

    // Load the interpreter, if any.
    let mut interp_base = 0usize;
    let mut entry = exec_base + header.entry as usize;

    if let Some(interp_ph) = phdrs.iter().find(|p| p.ptype == PT_INTERP) {
        let mut name = alloc::vec![0u8; interp_ph.filesz as usize];
        file.read_at(interp_ph.offset as usize, &mut name)?;
        // Trim the trailing NUL.
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let interp_path = String::from_utf8_lossy(&name[..end]).into_owned();

        let interp_inode = fs::path::resolve(&interp_path, true).map_err(|e| {
            crate::warn!("interpreter {} not found", interp_path);
            e
        })?;
        let interp_file = Arc::new(File::with_path(
            interp_inode,
            OpenFlags::RDONLY,
            &interp_path,
        ));
        let interp_header = read_header(&interp_file)?;
        let interp_phdrs = read_phdrs(&interp_file, &interp_header)?;
        interp_base = if interp_header.etype == ET_DYN {
            mm::USER_INTERP_BASE
        } else {
            0
        };
        map_segments(aspace, &interp_file, &interp_phdrs, interp_base, "interp")?;
        entry = interp_base + interp_header.entry as usize;
        crate::trace!(
            "loaded interpreter {} at {:#x}, entry {:#x}",
            interp_path,
            interp_base,
            entry
        );
    }

    Ok(LoadedImage {
        entry,
        exec_entry: exec_base + header.entry as usize,
        exec_base,
        interp_base,
        phdr_addr,
        phentsize: header.phentsize as usize,
        phnum: header.phnum as usize,
        brk: mm::page_up(max_end),
    })
}

/// Build the initial user stack: argv, envp, and the auxiliary vector.
///
/// The layout the ABI requires, from low to high:
/// ```text
///   sp ->  argc
///          argv[0..argc]  NULL
///          envp[0..]      NULL
///          auxv pairs     AT_NULL
///          (strings, AT_RANDOM bytes, ...)
/// ```
pub fn setup_stack(
    aspace: &mut AddrSpace,
    image: &LoadedImage,
    argv: &[Vec<u8>],
    envp: &[Vec<u8>],
    exec_path: &str,
) -> fs::Result<usize> {
    let stack_top = mm::USER_STACK_TOP;
    let stack_bottom = stack_top - mm::USER_STACK_SIZE;
    aspace.map_region(
        stack_bottom,
        stack_top,
        Prot::READ | Prot::WRITE,
        Backing::Anon,
        false,
        "[stack]",
    );
    // The top pages get written immediately; fault them in now so the writes
    // below (which go through raw pointers) can't fail.
    if !aspace.populate(stack_top - 64 * PAGE_SIZE, stack_top, true) {
        crate::bail!(ENOMEM);
    }

    let mut sp = stack_top;

    // Helper to push a byte blob and return its address.
    let mut push_bytes = |sp: &mut usize, data: &[u8]| -> usize {
        *sp -= data.len();
        // Keep 8-byte alignment for anything we may read as words later.
        *sp &= !0x7;
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), *sp as *mut u8, data.len());
        }
        *sp
    };

    // Strings first, at the top of the stack.
    let mut argv_addrs = Vec::with_capacity(argv.len());
    for arg in argv {
        let mut buf = arg.clone();
        buf.push(0);
        argv_addrs.push(push_bytes(&mut sp, &buf));
    }
    let mut envp_addrs = Vec::with_capacity(envp.len());
    for env in envp {
        let mut buf = env.clone();
        buf.push(0);
        envp_addrs.push(push_bytes(&mut sp, &buf));
    }

    // AT_EXECFN: the path we were invoked with.
    let mut execfn_buf = exec_path.as_bytes().to_vec();
    execfn_buf.push(0);
    let execfn_addr = push_bytes(&mut sp, &execfn_buf);

    // AT_PLATFORM.
    let platform_addr = push_bytes(&mut sp, b"riscv64\0");

    // AT_RANDOM: 16 bytes of randomness. musl uses these to seed the stack
    // guard and its malloc, so they must be readable and ideally not constant.
    let mut random = [0u8; 16];
    fs::device::fill_random(&mut random);
    let random_addr = push_bytes(&mut sp, &random);

    // Build the auxiliary vector.
    let task = crate::task::current();
    let aux: [(usize, usize); 16] = [
        (auxv::AT_PHDR, image.phdr_addr),
        (auxv::AT_PHENT, image.phentsize),
        (auxv::AT_PHNUM, image.phnum),
        (auxv::AT_PAGESZ, PAGE_SIZE),
        (auxv::AT_BASE, image.interp_base),
        (auxv::AT_FLAGS, 0),
        (auxv::AT_ENTRY, image.exec_entry),
        (auxv::AT_UID, task.uid() as usize),
        (auxv::AT_EUID, task.euid() as usize),
        (auxv::AT_GID, task.gid() as usize),
        (auxv::AT_EGID, task.egid() as usize),
        (auxv::AT_SECURE, 0),
        (auxv::AT_CLKTCK, crate::time::TICK_HZ as usize),
        (auxv::AT_RANDOM, random_addr),
        (auxv::AT_PLATFORM, platform_addr),
        (auxv::AT_EXECFN, execfn_addr),
    ];

    // Compute the total size of the word-sized region so we can align `sp` such
    // that the final sp is 16-byte aligned, as the ABI requires.
    let words = 1                      // argc
        + argv_addrs.len() + 1         // argv + NULL
        + envp_addrs.len() + 1         // envp + NULL
        + aux.len() * 2 + 2; // auxv + AT_NULL pair
    let bytes = words * core::mem::size_of::<usize>();
    sp = (sp - bytes) & !0xf;

    // Now write everything out from `sp` upward.
    let mut cursor = sp;
    let mut push_word = |cursor: &mut usize, value: usize| {
        unsafe { core::ptr::write(*cursor as *mut usize, value) };
        *cursor += core::mem::size_of::<usize>();
    };

    push_word(&mut cursor, argv_addrs.len());
    for &addr in &argv_addrs {
        push_word(&mut cursor, addr);
    }
    push_word(&mut cursor, 0);
    for &addr in &envp_addrs {
        push_word(&mut cursor, addr);
    }
    push_word(&mut cursor, 0);
    for (key, value) in aux {
        push_word(&mut cursor, key);
        push_word(&mut cursor, value);
    }
    push_word(&mut cursor, auxv::AT_NULL);
    push_word(&mut cursor, 0);

    Ok(sp)
}

/// Install the sigreturn trampoline page in an address space.
///
/// The page holds `li a7, 139; ecall` so a handler returning without an
/// `sa_restorer` still lands in `rt_sigreturn`.
pub fn map_sigreturn_trampoline(aspace: &mut AddrSpace) -> fs::Result<usize> {
    const TRAMPOLINE_VA: usize = mm::USER_STACK_TOP + PAGE_SIZE;
    aspace.map_region(
        TRAMPOLINE_VA,
        TRAMPOLINE_VA + PAGE_SIZE,
        Prot::READ | Prot::EXEC,
        Backing::Anon,
        false,
        "[sigreturn]",
    );
    // Populate it with the instructions, which needs write access first.
    aspace.protect_range(
        TRAMPOLINE_VA,
        TRAMPOLINE_VA + PAGE_SIZE,
        Prot::READ | Prot::WRITE | Prot::EXEC,
    );
    if !aspace.populate(TRAMPOLINE_VA, TRAMPOLINE_VA + PAGE_SIZE, true) {
        crate::bail!(ENOMEM);
    }
    // li a7, 139  ->  addi a7, zero, 139   = 0x08b00893
    // ecall                                 = 0x00000073
    let code: [u32; 2] = [0x08b0_0893, 0x0000_0073];
    unsafe {
        core::ptr::copy_nonoverlapping(
            code.as_ptr() as *const u8,
            TRAMPOLINE_VA as *mut u8,
            core::mem::size_of_val(&code),
        );
    }
    aspace.protect_range(
        TRAMPOLINE_VA,
        TRAMPOLINE_VA + PAGE_SIZE,
        Prot::READ | Prot::EXEC,
    );
    crate::signal::set_trampoline(TRAMPOLINE_VA);
    Ok(TRAMPOLINE_VA)
}

/// Interpret a `#!` script header, returning the interpreter argv to prepend.
pub fn parse_shebang(inode: &InodeRef) -> Option<Vec<Vec<u8>>> {
    let mut buf = [0u8; 256];
    let n = inode.read_at(0, &mut buf).ok()?;
    if n < 2 || &buf[..2] != b"#!" {
        return None;
    }
    let line_end = buf[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
    let line = &buf[2..line_end];
    let mut parts: Vec<Vec<u8>> = Vec::new();
    // Linux splits the shebang into at most the interpreter plus one argument.
    let trimmed: &[u8] = {
        let start = line.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(line.len());
        &line[start..]
    };
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.iter().position(|&b| b == b' ' || b == b'\t') {
        Some(idx) => {
            parts.push(trimmed[..idx].to_vec());
            let rest = &trimmed[idx + 1..];
            let start = rest
                .iter()
                .position(|&b| b != b' ' && b != b'\t')
                .unwrap_or(rest.len());
            let arg = &rest[start..];
            if !arg.is_empty() {
                parts.push(arg.to_vec());
            }
        }
        None => parts.push(trimmed.to_vec()),
    }
    Some(parts)
}

/// Load a program into the current address space, replacing its contents
/// (`execve`). Returns the new trap context.
pub fn exec(
    path: &str,
    argv: &[Vec<u8>],
    envp: &[Vec<u8>],
) -> fs::Result<TrapContext> {
    let task = crate::task::current();

    // Resolve first so a failure leaves the old image intact.
    let inode = fs::path::resolve(path, true)?;
    if inode.kind() == fs::InodeKind::Dir {
        crate::bail!(EACCES);
    }

    // Handle `#!` scripts by re-targeting at the interpreter.
    let (real_path, real_argv) = match parse_shebang(&inode) {
        Some(interp) => {
            let interp_path = String::from_utf8_lossy(&interp[0]).into_owned();
            let mut new_argv = interp.clone();
            new_argv.push(path.as_bytes().to_vec());
            new_argv.extend_from_slice(&argv[1.min(argv.len())..]);
            (interp_path, new_argv)
        }
        None => (path.to_string(), argv.to_vec()),
    };

    let inode = if real_path == path {
        inode
    } else {
        fs::path::resolve(&real_path, true)?
    };

    // Everything from here on modifies the process, so failures are fatal.
    let mut aspace = task.aspace.lock();
    aspace.clear_user();

    let image = load_elf_inode(&mut aspace, inode, &real_path)?;
    map_sigreturn_trampoline(&mut aspace)?;

    // The heap starts after the executable's segments.
    aspace.brk = image.brk;
    aspace.brk_start = image.brk;
    aspace.map_region(
        image.brk,
        image.brk,
        Prot::READ | Prot::WRITE,
        Backing::Anon,
        false,
        "[heap]",
    );

    let sp = setup_stack(&mut aspace, &image, &real_argv, envp, &real_path)?;
    drop(aspace);

    // Update process metadata.
    *task.group.exe.write() = real_path.clone();
    *task.group.cmdline.write() = real_argv
        .iter()
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let base_name = real_path.rsplit('/').next().unwrap_or(&real_path);
    *task.comm.write() = base_name.chars().take(15).collect();

    // `execve` resets signal handlers to default (keeping ignored ones) and
    // clears the alternate stack.
    {
        let mut actions = task.group.actions.lock();
        for (sig, action) in actions.iter_mut().enumerate() {
            if action.handler != crate::signal::SIG_IGN || sig == 0 {
                *action = crate::signal::SigAction::default();
            } else {
                // Keep SIG_IGN, but drop the flags and mask.
                *action = crate::signal::SigAction {
                    handler: crate::signal::SIG_IGN,
                    flags: 0,
                    restorer: 0,
                    mask: crate::signal::SigSet::EMPTY,
                };
            }
        }
    }
    {
        let mut signals = task.signals.lock();
        signals.altstack = crate::signal::SigAltStack::default();
    }
    task.files.lock().close_on_exec();
    task.clear_child_tid.store(0, core::sync::atomic::Ordering::Relaxed);

    crate::trace!(
        "exec {} entry={:#x} sp={:#x} phdr={:#x} interp_base={:#x}",
        real_path,
        image.entry,
        sp,
        image.phdr_addr,
        image.interp_base,
    );

    Ok(TrapContext::new_user(image.entry, sp, task.kernel_sp()))
}

use alloc::string::ToString;
