//! execve: build a fresh address space and user stack for an ELF program.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::{elf, Task};
use crate::abi::*;
use crate::config::*;
use crate::fs::file::File;
use crate::fs::vfs::Dentry;
use crate::mm::addrspace::{AddressSpace, Prot, Vma};
use crate::mm::uaccess::copy_to_user_mm;
use crate::sync::SpinLock;
use crate::trap::TrapFrame;

pub struct ExecImage {
    pub mm: Arc<SpinLock<AddressSpace>>,
    pub entry: usize,
    pub sp: usize,
    pub path: String,
}

fn open_exec(cwd: &Arc<Dentry>, path: &str) -> Result<Arc<File>, i32> {
    let f = crate::fs::open(cwd, path, O_RDONLY, 0)?;
    let st = f.stat()?;
    if st.st_mode & S_IFMT == S_IFDIR {
        return Err(EACCES);
    }
    if st.st_mode & S_IFMT != S_IFREG {
        return Err(EACCES);
    }
    Ok(f)
}

/// Prepare an executable image. Handles `#!` scripts by recursion.
pub fn load_image(
    cwd: &Arc<Dentry>,
    path: &str,
    argv: Vec<Vec<u8>>,
    envp: Vec<Vec<u8>>,
    depth: usize,
) -> Result<ExecImage, i32> {
    if depth > 4 {
        return Err(ELOOP);
    }
    let file = open_exec(cwd, path)?;
    let mut magic = [0u8; 128];
    let n = file.pread(&mut magic, 0)?;
    if n >= 2 && &magic[0..2] == b"#!" {
        // shebang: "#!interp [arg]\n"
        let line_end = magic[..n].iter().position(|&b| b == b'\n').unwrap_or(n);
        let line = String::from_utf8_lossy(&magic[2..line_end]).into_owned();
        let mut parts = line.split_whitespace();
        let interp = parts.next().ok_or(ENOEXEC)?;
        let mut new_argv: Vec<Vec<u8>> = Vec::new();
        new_argv.push(interp.as_bytes().to_vec());
        let rest: Vec<&str> = parts.collect();
        if !rest.is_empty() {
            new_argv.push(rest.join(" ").into_bytes());
        }
        new_argv.push(path.as_bytes().to_vec());
        new_argv.extend(argv.into_iter().skip(1));
        return load_image(cwd, &String::from(interp), new_argv, envp, depth + 1);
    }
    let info = elf::parse(&file)?;
    let mm = Arc::new(SpinLock::new(AddressSpace::new()));
    let base_hint = if info.ehdr.e_type == elf::ET_DYN { PIE_BASE } else { 0 };
    let loaded = elf::load(&mm, &file, &info, base_hint)?;

    let (entry, interp_base) = match &info.interp {
        Some(ipath) => {
            let ifile = open_exec(cwd, ipath)?;
            let iinfo = elf::parse(&ifile)?;
            let il = elf::load(&mm, &ifile, &iinfo, INTERP_BASE)?;
            (il.entry, il.base)
        }
        None => (loaded.entry, 0),
    };

    {
        let mut a = mm.lock();
        // heap
        let brk = crate::mm::addrspace::page_up(loaded.end) + PAGE_SIZE;
        a.brk_start = brk;
        a.brk = brk;
        // stack
        a.insert_vma(Vma {
            start: USER_STACK_TOP - USER_STACK_SIZE,
            end: USER_STACK_TOP,
            prot: Prot::R | Prot::W,
            shared: false,
            file: None,
            grows_down: true,
        });
        // signal return trampoline page
        a.insert_vma(Vma {
            start: SIGRET_TRAMPOLINE,
            end: SIGRET_TRAMPOLINE + PAGE_SIZE,
            prot: Prot::R | Prot::W | Prot::X,
            shared: false,
            file: None,
            grows_down: false,
        });
    }
    // li a7, 139 ; ecall   (rt_sigreturn)
    let tramp: [u8; 8] = [0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];
    copy_to_user_mm(&mm, SIGRET_TRAMPOLINE, &tramp)?;
    mm.lock().mprotect(SIGRET_TRAMPOLINE, PAGE_SIZE, Prot::R | Prot::X)?;

    // Build the initial stack.
    let (uid, gid) = match super::try_current() {
        Some(t) => (
            t.uid.load(core::sync::atomic::Ordering::Relaxed) as usize,
            t.gid.load(core::sync::atomic::Ordering::Relaxed) as usize,
        ),
        None => (0, 0),
    };
    let mut random = [0u8; 16];
    crate::fs::devices::fill_random(&mut random);

    let mut sp = USER_STACK_TOP;
    let push_bytes = |mm: &Arc<SpinLock<AddressSpace>>, sp: &mut usize, data: &[u8]| -> Result<usize, i32> {
        *sp -= data.len();
        copy_to_user_mm(mm, *sp, data)?;
        Ok(*sp)
    };
    let execfn_addr = push_bytes(&mm, &mut sp, &[path.as_bytes(), &[0]].concat())?;
    let platform_addr = push_bytes(&mm, &mut sp, b"riscv64\0")?;
    let mut envp_addrs = Vec::new();
    for e in envp.iter().rev() {
        let a = push_bytes(&mm, &mut sp, &[e.as_slice(), &[0]].concat())?;
        envp_addrs.push(a);
    }
    envp_addrs.reverse();
    let mut argv_addrs = Vec::new();
    for a in argv.iter().rev() {
        let addr = push_bytes(&mm, &mut sp, &[a.as_slice(), &[0]].concat())?;
        argv_addrs.push(addr);
    }
    argv_addrs.reverse();
    sp &= !15;
    let random_addr = push_bytes(&mm, &mut sp, &random)?;
    sp &= !15;

    let auxv: Vec<(usize, usize)> = alloc::vec![
        (AT_PHDR, loaded.phdr_addr),
        (AT_PHENT, core::mem::size_of::<elf::Phdr>()),
        (AT_PHNUM, info.ehdr.phnum as usize),
        (AT_PAGESZ, PAGE_SIZE),
        (AT_BASE, interp_base),
        (AT_FLAGS, 0),
        (AT_ENTRY, loaded.entry),
        (AT_UID, uid),
        (AT_EUID, uid),
        (AT_GID, gid),
        (AT_EGID, gid),
        (AT_SECURE, 0),
        (AT_RANDOM, random_addr),
        (AT_HWCAP, 0x112d), // IMAFDC
        (AT_CLKTCK, 100),
        (AT_PLATFORM, platform_addr),
        (AT_EXECFN, execfn_addr),
        (AT_NULL, 0),
    ];
    let words = 1 + (argv_addrs.len() + 1) + (envp_addrs.len() + 1) + auxv.len() * 2;
    let mut block: Vec<u8> = Vec::with_capacity(words * 8);
    let push = |b: &mut Vec<u8>, v: usize| b.extend_from_slice(&v.to_le_bytes());
    push(&mut block, argv_addrs.len());
    for a in &argv_addrs {
        push(&mut block, *a);
    }
    push(&mut block, 0);
    for e in &envp_addrs {
        push(&mut block, *e);
    }
    push(&mut block, 0);
    for (k, v) in &auxv {
        push(&mut block, *k);
        push(&mut block, *v);
    }
    sp -= block.len();
    sp &= !15;
    copy_to_user_mm(&mm, sp, &block)?;

    Ok(ExecImage { mm, entry, sp, path: String::from(path) })
}

/// Replace the current task's image with `img`.
pub fn commit(task: &Arc<Task>, img: ExecImage, name: String) {
    // Close close-on-exec fds.
    let closed = task.fds().lock().close_on_exec();
    drop(closed);
    // Reset signal handlers (ignored ones stay ignored).
    {
        let sig = task.sig();
        let mut sig = sig.lock();
        for a in sig.actions.iter_mut() {
            if a.handler != SIG_IGN {
                *a = KSigAction::default();
            }
        }
    }
    {
        let mut inner = task.inner.lock();
        inner.name = name;
        inner.exe_path = img.path.clone();
        inner.sigaltstack = StackT::default();
        inner.clear_child_tid = 0;
        inner.robust_list = 0;
        inner.syscall_restart = None;
    }
    // Unshare signal handlers / fd table if they were shared (threads) — not applicable.
    task.set_mm(img.mm);
    super::sched::reload_mm(task);
    let tf = task.tf();
    *tf = TrapFrame::new_user(img.entry, img.sp, task.kstack_top());
}
