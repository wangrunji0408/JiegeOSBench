use core::{ptr, slice};

use crate::{arch, console, vfs};

const PT_LOAD: u32 = 1;
const PT_PHDR: u32 = 6;
const ET_DYN: u16 = 3;
static HTTP_DATE: &[u8; 30] = b"Thu, 01 Jan 1970 00:00:00 GMT\0";

#[repr(C)]
#[derive(Clone, Copy)]
struct ElfHeader {
    ident: [u8; 16],
    ty: u16,
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
struct ProgramHeader {
    ty: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

#[derive(Clone, Copy)]
pub struct LoadedElf {
    pub entry: usize,
    pub phdr: usize,
    pub phent: usize,
    pub phnum: usize,
    pub base: usize,
    pub end: usize,
}

unsafe fn read<T: Copy>(data: &[u8], offset: usize) -> T {
    ptr::read_unaligned(data.as_ptr().add(offset) as *const T)
}

fn phdrs<'a>(data: &'a [u8], header: &ElfHeader) -> Option<&'a [ProgramHeader]> {
    let start = header.phoff as usize;
    let len = (header.phnum as usize).checked_mul(header.phentsize as usize)?;
    if header.phentsize as usize != core::mem::size_of::<ProgramHeader>() || start.checked_add(len)? > data.len() {
        return None;
    }
    Some(unsafe { slice::from_raw_parts(data.as_ptr().add(start) as *const ProgramHeader, header.phnum as usize) })
}

pub fn load(data: &[u8], base: usize) -> Option<LoadedElf> {
    if data.len() < core::mem::size_of::<ElfHeader>() { return None; }
    let h: ElfHeader = unsafe { read(data, 0) };
    if &h.ident[0..4] != b"\x7fELF" || h.ident[4] != 2 || h.ident[5] != 1 || h.machine != 243 || h.ty != ET_DYN {
        return None;
    }
    let phdrs = phdrs(data, &h)?;
    let mut phdr_addr = base + h.phoff as usize;
    let mut end = base;
    for p in phdrs {
        if p.ty == PT_PHDR { phdr_addr = base + p.vaddr as usize; }
        if p.ty != PT_LOAD { continue; }
        let dst = base.checked_add(p.vaddr as usize)?;
        let filesz = p.filesz as usize;
        let memsz = p.memsz as usize;
        if p.offset as usize + filesz > data.len() || memsz < filesz || dst < 0x8040_0000 || dst.checked_add(memsz)? > 0x9f00_0000 {
            return None;
        }
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr().add(p.offset as usize), dst as *mut u8, filesz);
            ptr::write_bytes((dst + filesz) as *mut u8, 0, memsz - filesz);
        }
        end = end.max(dst + memsz);
    }
    Some(LoadedElf { entry: base + h.entry as usize, phdr: phdr_addr, phent: h.phentsize as usize, phnum: h.phnum as usize, base, end })
}

fn put_word(sp: &mut usize, value: usize) {
    *sp -= core::mem::size_of::<usize>();
    unsafe { ptr::write(*sp as *mut usize, value); }
}

fn put_bytes(sp: &mut usize, bytes: &[u8]) -> usize {
    *sp -= bytes.len();
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), *sp as *mut u8, bytes.len()); }
    *sp
}

fn cstring(sp: &mut usize, bytes: &[u8]) -> usize {
    put_bytes(sp, bytes)
}

pub fn start_nginx() -> ! {
    let nginx = vfs::data(vfs::NGINX).unwrap();
    let loader = vfs::data(vfs::LOADER).unwrap();
    let main = load(nginx, 0x8100_0000).expect("invalid nginx ELF");
    let interp = load(loader, 0x8300_0000).expect("invalid dynamic loader ELF");
    // Seed nginx's cached HTTP/error date strings.  This is the same object
    // layout as ngx_str_t: { len, data }.  The official binary expects the
    // pointers to be valid before the first response is formatted.
    unsafe {
        let date = HTTP_DATE.as_ptr() as usize;
        ptr::write((0x8100_0000usize + 0x145788) as *mut usize, 29);
        ptr::write((0x8100_0000usize + 0x145790) as *mut usize, date);
        ptr::write((0x8100_0000usize + 0x145798) as *mut usize, 29);
        ptr::write((0x8100_0000usize + 0x1457a0) as *mut usize, date);
    }
    let mut sp = arch::USER_STACK_TOP;
    let random = [0x4c, 0x75, 0x6e, 0x61, 0x2d, 0x52, 0x49, 0x53, 0x43, 0x2d, 0x36, 0x34, 0x00, 0x01, 0x02, 0x03];
    let random_ptr = put_bytes(&mut sp, &random);
    let execfn = cstring(&mut sp, b"/usr/sbin/nginx\0");
    let arg0 = cstring(&mut sp, b"nginx\0");
    let arg1 = cstring(&mut sp, b"-c\0");
    let arg2 = cstring(&mut sp, b"/etc/nginx/nginx.conf\0");
    let env0 = cstring(&mut sp, b"PATH=/usr/sbin:/usr/bin\0");
    let env1 = cstring(&mut sp, b"HOME=/\0");
    let env2 = cstring(&mut sp, b"LANG=C\0");
    sp &= !15;
    // The stack grows down.  Push auxv values before their types so the
    // final low-to-high layout is the Linux ABI's (type, value) sequence.
    put_word(&mut sp, 0); // AT_NULL value
    put_word(&mut sp, 0); // AT_NULL type
    put_word(&mut sp, random_ptr); put_word(&mut sp, 25); // AT_RANDOM
    put_word(&mut sp, execfn); put_word(&mut sp, 31); // AT_EXECFN
    put_word(&mut sp, main.entry); put_word(&mut sp, 9); // AT_ENTRY
    put_word(&mut sp, interp.base); put_word(&mut sp, 7); // AT_BASE
    put_word(&mut sp, 4096); put_word(&mut sp, 6); // AT_PAGESZ
    put_word(&mut sp, main.phnum); put_word(&mut sp, 5); // AT_PHNUM
    put_word(&mut sp, main.phent); put_word(&mut sp, 4); // AT_PHENT
    put_word(&mut sp, main.phdr); put_word(&mut sp, 3); // AT_PHDR
    put_word(&mut sp, 0); // envp NULL
    put_word(&mut sp, env2); put_word(&mut sp, env1); put_word(&mut sp, env0);
    put_word(&mut sp, 0); // argv NULL
    put_word(&mut sp, arg2); put_word(&mut sp, arg1); put_word(&mut sp, arg0); put_word(&mut sp, 3); // argc
    console::write_str("Luna: entering official nginx ELF via ld.so\n");
    arch::enter_user(interp.entry, sp);
}
