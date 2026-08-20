//! ELF64 加载器 + 用户初始栈构造（Linux ABI）

use alloc::string::String;
use alloc::vec::Vec;
use crate::errno::{Errno, Ret};
use crate::page::{self, PTE_A, PTE_D, PTE_R, PTE_U, PTE_W, PTE_X};
use crate::pmm;
use crate::proc;
use crate::trap::TrapFrame;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;

const EM_RISCV: u16 = 243;

#[repr(C)]
struct ElfHeader {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    // 后面节区相关不需要
}

#[repr(C)]
struct Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

fn read<T>(data: &[u8], off: usize) -> Option<&T> {
    if off + core::mem::size_of::<T>() > data.len() {
        return None;
    }
    unsafe { Some(&*(data.as_ptr().add(off) as *const T)) }
}

pub struct LoadedImage {
    pub entry: usize,
    pub phdr_va: usize,
    pub phnum: usize,
    pub phent: usize,
    pub brk_end: usize,
}

const PROG_BASE: usize = 0x1000_0000_00; // 主程序 PIE 基址
const LD_BASE: usize = 0x2000_0000_00; // 动态链接器基址
pub const STACK_TOP: usize = 0x3fff_ffff_f000;
const STACK_SIZE: usize = 16 << 20; // 16MB 栈 VMA（lazy 分配）

/// 把段映射进用户页表并复制内容。返回段尾（memsz 结束）VA。
fn load_segment(data: &[u8], seg_va: usize, seg_len: usize, file_off: usize, file_len: usize, pflags: u32) -> Ret<()> {
    let start = seg_va & !0xfff;
    let end = (seg_va + seg_len + 0xfff) & !0xfff;
    let mut flags = PTE_U | PTE_A | PTE_D;
    if pflags & 4 != 0 {
        flags |= PTE_R;
    }
    if pflags & 2 != 0 {
        flags |= PTE_W;
    }
    if pflags & 1 != 0 {
        flags |= PTE_X;
    }
    if !proc::add_vma(start, end, flags) {
        return Err(Errno::Enomem);
    }
    let root = proc::current_page_table_root();
    // 逐页分配、映射、复制
    let mut va = start;
    while va < end {
        let pa = pmm::alloc_page().ok_or(Errno::Enomem)?;
        page::map_4k(root, va, pa, flags | page::PTE_V);
        va += 0x1000;
    }
    // 复制文件内容（物理地址写入，alloc_page 已清零 BSS）
    if file_len > 0 {
        let mut copied = 0usize;
        while copied < file_len {
            let va = seg_va + copied;
            let off_in_page = va & 0xfff;
            let chunk = core::cmp::min(0x1000 - off_in_page, file_len - copied);
            let (_, pte_flags) = page::lookup(root, va & !0xfff).expect("page must be mapped");
            let pa = page::lookup(root, va & !0xfff).expect("page must be mapped").0;
            let _ = pte_flags;
            let dst = (pa + off_in_page) as *mut u8;
            let src = &data[file_off + copied..file_off + copied + chunk];
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst, chunk);
            }
            copied += chunk;
        }
    }
    Ok(())
}

fn load_one_elf(data: &[u8], base: usize) -> Ret<LoadedImage> {
    let ehdr = read::<ElfHeader>(data, 0).ok_or(Errno::Enoexec)?;
    if &ehdr.e_ident[0..4] != b"\x7fELF" {
        return Err(Errno::Enoexec);
    }
    if ehdr.e_ident[4] != 2 {
        return Err(Errno::Enoexec); // 不是 ELF64
    }
    if ehdr.e_machine != EM_RISCV {
        return Err(Errno::Enoexec);
    }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(Errno::Enoexec);
    }

    let phnum = ehdr.e_phnum as usize;
    let phent = ehdr.e_phentsize as usize;
    let mut max_end: usize = 0;
    let mut interp: Option<String> = None;

    for i in 0..phnum {
        let ph = read::<Phdr>(data, ehdr.e_phoff as usize + i * phent).ok_or(Errno::Enoexec)?;
        match ph.p_type {
            PT_LOAD => {
                let seg_va = base + ph.p_vaddr as usize;
                load_segment(
                    data,
                    seg_va,
                    ph.p_memsz as usize,
                    ph.p_offset as usize,
                    ph.p_filesz as usize,
                    ph.p_flags,
                )?;
                let seg_end = seg_va + ph.p_memsz as usize;
                if seg_end > max_end {
                    max_end = seg_end;
                }
            }
            PT_INTERP => {
                let off = ph.p_offset as usize;
                let len = ph.p_filesz as usize;
                let bytes = &data[off..off + len];
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
                interp = Some(String::from_utf8_lossy(&bytes[..end]).into_owned());
            }
            _ => {}
        }
    }

    Ok(LoadedImage {
        entry: base + ehdr.e_entry as usize,
        phdr_va: base + ehdr.e_phoff as usize,
        phnum,
        phent,
        brk_end: (max_end + 0xfff) & !0xfff,
    })
}

/// 加载主程序（含动态链接器）并设置初始用户栈 / trapframe
pub fn load_program(data: &[u8], argv: &[&str]) -> Ret<()> {
    let main = load_one_elf(data, PROG_BASE)?;

    // 动态链接器
    let ld_image = if let Some(interp) = interp_path(data) {
        let (ld_data, _) = crate::vfs::open_read(&interp).ok_or(Errno::Enoent)?;
        let bytes = match ld_data {
            crate::vfs::FileData::Static(b) => b,
            crate::vfs::FileData::Tmp(v) => {
                // ld.so 必须在静态 rootfs；tmpfs 理论上不会出现
                let mut t = Vec::new();
                t.extend_from_slice(&v.borrow());
                return Err(Errno::Enoexec);
            }
        };
        Some(load_one_elf(bytes, LD_BASE)?)
    } else {
        None
    };

    let ld = ld_image;
    let entry = if ld.is_some() {
        // 动态链接：入口在 ld.so，它收到栈上的信息后跳主程序
        ld.as_ref().unwrap().entry
    } else {
        main.entry
    };
    let ld_base = ld.as_ref().map(|l| LD_BASE).unwrap_or(0);

    // 栈 VMA
    proc::add_vma(STACK_TOP - STACK_SIZE, STACK_TOP, PTE_U | PTE_R | PTE_W | PTE_A | PTE_D)
        .then_some(())
        .ok_or(Errno::Enomem)?;

    let envp: [&str; 3] = ["HOME=/", "PATH=/usr/sbin:/usr/bin:/sbin:/bin", "LANG=C.UTF-8"];

    let sp = build_user_stack(argv, &envp, &main, ld_base, argv[0])?;

    // 设置 brk
    {
        let proc = proc::current();
        proc.brk = main.brk_end;
        // brk VMA 预留：不单独建，brk() 扩展时动态加
    }

    // 配置 trapframe
    let frame = unsafe {
        let ptr = crate::trap::TRAPFRAME_ADDR as *mut TrapFrame;
        &mut *ptr
    };
    frame.x = [0; 32];
    frame.f = [0; 32];
    frame.fcsr = 0;
    frame.sepc = entry as u64;
    frame.x[2] = sp as u64; // sp
    frame.sstatus = crate::trap::SSTATUS_SPIE | crate::trap::SSTATUS_SUM | crate::trap::SSTATUS_FS_INITIAL;
    Ok(())
}

fn interp_path(data: &[u8]) -> Option<String> {
    let ehdr = read::<ElfHeader>(data, 0)?;
    for i in 0..ehdr.e_phnum as usize {
        let ph = read::<Phdr>(data, ehdr.e_phoff as usize + i * ehdr.e_phentsize as usize)?;
        if ph.p_type == PT_INTERP {
            let off = ph.p_offset as usize;
            let len = ph.p_filesz as usize;
            let bytes = &data[off..off + len];
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
            return Some(String::from_utf8_lossy(&bytes[..end]).into_owned());
        }
    }
    None
}

/// 构造初始用户栈，返回初始 sp
fn build_user_stack(argv: &[&str], envp: &[&str], main: &LoadedImage, ld_base: usize, execfn: &str) -> Ret<usize> {
    let root = proc::current_page_table_root();

    // 常量字符串与随机数
    let platform = "riscv64";
    let mut random = [0u8; 16];
    crate::syscall::fill_random(&mut random);

    // 1. 计算需要的总大小
    let argc = argv.len();
    let mut strings_size = platform.len() + 1 + execfn.len() + 1 + 16;
    for s in argv.iter().chain(envp.iter()) {
        strings_size += s.len() + 1;
    }
    let ptrs_size = 8 * (1 + argc + 1 + envp.len() + 1);
    // auxv 条目
    let aux_count = 14;
    let aux_size = 16 * aux_count;
    let total = (ptrs_size + aux_size + strings_size + 31) & !15;

    let sp0 = (STACK_TOP - total) & !15;
    let stack_bottom = sp0 & !0xfff;

    // 2. 映射栈页（物理连续分配，便于直接写入）
    let npages = (STACK_TOP - stack_bottom + 0xfff) / 0x1000;
    let mut pages = Vec::new();
    for i in 0..npages {
        let pa = pmm::alloc_page().ok_or(Errno::Enomem)?;
        let va = stack_bottom + i * 0x1000;
        page::map_4k(root, va, pa, PTE_U | PTE_R | PTE_W | PTE_A | PTE_D | page::PTE_V);
        pages.push(pa);
    }
    let va_to_pa = |va: usize| -> usize {
        let i = (va - stack_bottom) / 0x1000;
        pages[i] + (va & 0xfff)
    };
    unsafe fn write_at(va_to_pa: &dyn Fn(usize) -> usize, va: usize, bytes: &[u8]) {
        let mut off = 0;
        while off < bytes.len() {
            let v = va + off;
            let pa = va_to_pa(v);
            let chunk = core::cmp::min(0x1000 - (v & 0xfff), bytes.len() - off);
            core::ptr::copy_nonoverlapping(bytes[off..off + chunk].as_ptr(), pa as *mut u8, chunk);
            off += chunk;
        }
    }

    // 3. 布局：低→高 = argc, argv ptrs, NULL, envp ptrs, NULL, auxv, strings, random
    //    strings 从高往低放
    let mut cursor = sp0; // 指针区起点
    let mut str_cursor = STACK_TOP; // 字符串区顶（向下分配）
    // 先分配字符串地址（栈顶向下）
    let execfn_off_place = {
        // 每个字符串：str_cursor -= len+1; 记录地址
        0usize
    };
    struct StrAlloc {
        cursor: usize,
    }
    impl StrAlloc {
        fn alloc(&mut self, s: &str) -> usize {
            self.cursor -= s.len() + 1;
            self.cursor
        }
    }
    let mut salloc = StrAlloc { cursor: STACK_TOP };
    let mut argv_addrs = Vec::new();
    for s in argv {
        argv_addrs.push(salloc.alloc(s));
    }
    let mut envp_addrs = Vec::new();
    for s in envp {
        envp_addrs.push(salloc.alloc(s));
    }
    let platform_addr = salloc.alloc(platform);
    let execfn_addr = salloc.alloc(execfn);
    let random_addr = salloc.alloc("0123456789abcdef"); // 16 字节占位（内容稍后写）
    // random_addr 实际是 16 字节区域

    // 写指针区
    let mut buf = Vec::new();
    buf.extend_from_slice(&(argc as u64).to_le_bytes());
    for a in &argv_addrs {
        buf.extend_from_slice(&(*a as u64).to_le_bytes());
    }
    buf.extend_from_slice(&0u64.to_le_bytes()); // argv NULL
    for a in &envp_addrs {
        buf.extend_from_slice(&(*a as u64).to_le_bytes());
    }
    buf.extend_from_slice(&0u64.to_le_bytes()); // envp NULL
    // auxv
    let mut aux: Vec<(u64, u64)> = Vec::new();
    aux.push((3, main.phdr_va as u64)); // AT_PHDR
    aux.push((4, main.phent as u64)); // AT_PHENT
    aux.push((5, main.phnum as u64)); // AT_PHNUM
    aux.push((6, 4096)); // AT_PAGESZ
    aux.push((7, ld_base as u64)); // AT_BASE
    aux.push((8, 0)); // AT_FLAGS
    aux.push((9, main.entry as u64)); // AT_ENTRY
    aux.push((11, 0)); // AT_UID
    aux.push((12, 0)); // AT_EUID
    aux.push((13, 0)); // AT_GID
    aux.push((14, 0)); // AT_EGID
    aux.push((15, platform_addr as u64)); // AT_PLATFORM
    aux.push((16, 0)); // AT_HWCAP
    aux.push((17, 100)); // AT_CLKTCK
    aux.push((23, 0)); // AT_SECURE
    aux.push((25, random_addr as u64)); // AT_RANDOM
    aux.push((31, execfn_addr as u64)); // AT_EXECFN
    aux.push((0, 0)); // AT_NULL
    for (k, v) in &aux {
        buf.extend_from_slice(&k.to_le_bytes());
        buf.extend_from_slice(&v.to_le_bytes());
    }
    unsafe { write_at(&va_to_pa, cursor, &buf) };
    cursor += buf.len();

    // 写字符串
    for (s, a) in argv.iter().zip(argv_addrs.iter()) {
        let mut b = Vec::new();
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        unsafe { write_at(&va_to_pa, *a, &b) };
    }
    for (s, a) in envp.iter().zip(envp_addrs.iter()) {
        let mut b = Vec::new();
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        unsafe { write_at(&va_to_pa, *a, &b) };
    }
    {
        let mut b = platform.as_bytes().to_vec();
        b.push(0);
        unsafe { write_at(&va_to_pa, platform_addr, &b) };
    }
    {
        let mut b = execfn.as_bytes().to_vec();
        b.push(0);
        unsafe { write_at(&va_to_pa, execfn_addr, &b) };
    }
    unsafe { write_at(&va_to_pa, random_addr, &random) };

    let _ = execfn_off_place;
    let _ = cursor;
    Ok(sp0)
}
