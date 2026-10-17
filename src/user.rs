// Loading a userspace program image and building its initial stack.
use crate::arch;
use crate::elf;
use crate::fs;
use crate::mm;

pub const AT_NULL: usize = 0;
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
pub const AT_PLATFORM: usize = 15;
pub const AT_HWCAP: usize = 16;
pub const AT_CLKTCK: usize = 17;
pub const AT_SECURE: usize = 23;
pub const AT_RANDOM: usize = 25;
pub const AT_HWCAP2: usize = 26;
pub const AT_EXECFN: usize = 31;
pub const AT_SYSINFO_EHDR: usize = 33;

pub static mut BRK_START: usize = 0;
pub static mut BRK_CUR: usize = 0;

fn push_bytes(sp: &mut usize, data: &[u8]) -> usize {
    *sp -= data.len();
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), *sp as *mut u8, data.len());
    }
    *sp
}

fn push_cstr(sp: &mut usize, s: &[u8]) -> usize {
    *sp -= 1;
    unsafe {
        *(*sp as *mut u8) = 0;
    }
    push_bytes(sp, s)
}

pub fn exec(path: &[u8], argv: &[&[u8]], envp: &[&[u8]]) -> ! {
    let data = match fs::read_file(path) {
        Some(d) => d,
        None => {
            crate::println!("[exec] cannot open {}", core::str::from_utf8(path).unwrap_or("?"));
            crate::sbi::shutdown(true);
        }
    };
    let interp = elf::interp_path(data);
    let main = match elf::load(data, mm::MAIN_BASE) {
        Ok(l) => l,
        Err(e) => {
            crate::println!("[exec] main load error: {}", e);
            crate::sbi::shutdown(true);
        }
    };
    crate::println!(
        "[exec] main base={:#x} entry={:#x} end={:#x}",
        main.base,
        main.entry,
        main.end
    );
    let mut interp_loaded = None;
    if let Some(ipath) = interp {
        crate::println!(
            "[exec] interpreter: {}",
            core::str::from_utf8(ipath).unwrap_or("?")
        );
        let idata = match fs::read_file(ipath) {
            Some(d) => d,
            None => {
                crate::println!("[exec] cannot open interpreter");
                crate::sbi::shutdown(true);
            }
        };
        match elf::load(idata, mm::INTERP_BASE) {
            Ok(l) => {
                crate::println!("[exec] interp base={:#x} entry={:#x}", l.base, l.entry);
                interp_loaded = Some(l);
            }
            Err(e) => {
                crate::println!("[exec] interp load error: {}", e);
                crate::sbi::shutdown(true);
            }
        }
    }
    unsafe {
        BRK_START = (main.end + 0xfff) & !0xfff;
        BRK_CUR = BRK_START;
    }

    let mut sp = mm::STACK_TOP;

    // strings
    let execfn = push_cstr(&mut sp, path);
    let platform = push_cstr(&mut sp, b"riscv64");
    let mut rnd = [0u8; 16];
    let t = arch::rdtime();
    for i in 0..16 {
        rnd[i] = ((t >> ((i % 8) * 8)) ^ (i as u64 * 37) ^ 0xa5) as u8;
    }
    let random = push_bytes(&mut sp, &rnd);
    let mut arg_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for a in argv {
        arg_ptrs.push(push_cstr(&mut sp, a));
    }
    let mut env_ptrs: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    for e in envp {
        env_ptrs.push(push_cstr(&mut sp, e));
    }

    sp &= !15;

    let hwcap: usize = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 8) | (1 << 12);

    let mut aux: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();
    aux.push((AT_PHDR, main.phdr));
    aux.push((AT_PHENT, main.phent));
    aux.push((AT_PHNUM, main.phnum));
    aux.push((AT_PAGESZ, 4096));
    aux.push((AT_BASE, interp_loaded.map(|l| l.base).unwrap_or(0)));
    aux.push((AT_FLAGS, 0));
    aux.push((AT_ENTRY, main.entry));
    aux.push((AT_UID, 0));
    aux.push((AT_EUID, 0));
    aux.push((AT_GID, 0));
    aux.push((AT_EGID, 0));
    aux.push((AT_PLATFORM, platform));
    aux.push((AT_HWCAP, hwcap));
    aux.push((AT_CLKTCK, 100));
    aux.push((AT_SECURE, 0));
    aux.push((AT_RANDOM, random));
    aux.push((AT_HWCAP2, 0));
    aux.push((AT_EXECFN, execfn));
    aux.push((AT_SYSINFO_EHDR, 0));
    aux.push((AT_NULL, 0));

    let mut words: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    words.push(arg_ptrs.len());
    words.extend_from_slice(&arg_ptrs);
    words.push(0);
    words.extend_from_slice(&env_ptrs);
    words.push(0);
    for &(k, v) in aux.iter() {
        words.push(k);
        words.push(v);
    }

    sp -= words.len() * 8;
    sp &= !15;
    for (i, &w) in words.iter().enumerate() {
        unsafe {
            core::ptr::write_unaligned((sp + i * 8) as *mut usize, w);
        }
    }

    crate::println!("[exec] stack={:#x} argc={}", sp, argv.len());
    let entry = interp_loaded.map(|l| l.entry).unwrap_or(main.entry);
    unsafe {
        arch::enter_user(entry, sp);
    }
}
