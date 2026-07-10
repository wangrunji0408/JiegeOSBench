/// ELF加载器
/// 支持静态和动态链接的riscv64 Linux ELF二进制

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use xmas_elf::{ElfFile, program::{self, Type, Flags}};

use crate::config::*;
use crate::mm::{MapArea, MapPerm, MapType, MemorySet};

/// ELF加载结果
pub struct ElfLoad {
    pub memory_set: MemorySet,
    /// 实际入口点（如果有interp，则是interp的入口）
    pub entry: usize,
    /// 用户栈顶
    pub user_sp: usize,
    /// 程序头表虚拟地址（传给aux vector）
    pub phdr_va: usize,
    /// 程序头数量
    pub phnum: usize,
    /// 程序本身的入口（原始，传给aux vector AT_ENTRY）
    pub elf_entry: usize,
    /// 程序基地址
    pub elf_base: usize,
    /// brk起始
    pub brk_start: usize,
}

/// 基址偏移（用于PIE）
/// 0x40000000 以上，VPN2=1，与VPN2=0的设备IO和VPN2=2的内核不冲突
const PIE_BASE: usize = 0x40000000;
const INTERP_BASE: usize = 0x50000000;

/// 加载ELF文件
pub fn load_elf(elf_data: &[u8], pid: usize) -> (MemorySet, usize, usize) {
    let result = load_elf_full(elf_data, pid);
    (result.memory_set, result.entry, result.user_sp)
}

pub fn load_elf_full(elf_data: &[u8], pid: usize) -> ElfLoad {
    let elf = ElfFile::new(elf_data).expect("invalid ELF");
    let header = elf.header;
    assert_eq!(header.pt2.machine().as_machine(), xmas_elf::header::Machine::RISC_V);
    assert_eq!(header.pt1.class(), xmas_elf::header::Class::SixtyFour);

    let mut memory_set = MemorySet::new_bare();
    map_kernel_into(&mut memory_set);

    // 判断是否是PIE
    let elf_type = header.pt2.type_().as_type();
    let is_pie = matches!(elf_type, xmas_elf::header::Type::SharedObject);

    let load_base = if is_pie { PIE_BASE } else { 0 };
    let elf_entry = header.pt2.entry_point() as usize + load_base;

    let mut max_va = 0usize;
    let mut phdr_va = 0usize;
    let phnum = elf.program_iter().count();
    let mut interp_path: Option<String> = None;

    // 第一遍：扫描PT_INTERP
    for ph in elf.program_iter() {
        if ph.get_type().unwrap() == Type::Interp {
            let offset = ph.offset() as usize;
            let size = ph.file_size() as usize;
            let path_bytes = &elf_data[offset..offset + size];
            // 去掉null terminator
            let path = core::str::from_utf8(path_bytes)
                .unwrap_or("")
                .trim_end_matches('\0');
            interp_path = Some(path.to_string());
            println!("[elf] Dynamic executable, interpreter: {}", path);
        }
    }

    // 加载主程序段
    for ph in elf.program_iter() {
        let ptype = ph.get_type().unwrap();

        if ptype == Type::Phdr {
            phdr_va = ph.virtual_addr() as usize + load_base;
            continue;
        }

        if ptype != Type::Load { continue; }

        let va_start = ph.virtual_addr() as usize + load_base;
        let va_end = va_start + ph.mem_size() as usize;
        let file_size = ph.file_size() as usize;
        let offset = ph.offset() as usize;

        let flags = ph.flags();
        let mut perm = MapPerm::U;
        if flags.is_read() { perm |= MapPerm::R; }
        if flags.is_write() { perm |= MapPerm::W; }
        if flags.is_execute() { perm |= MapPerm::X; }

        let va_start_aligned = va_start & !(PAGE_SIZE - 1);
        let va_end_aligned = (va_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        if va_end > max_va { max_va = va_end; }

        let mut area = MapArea::new(va_start_aligned, va_end_aligned, MapType::Framed, perm);
        area.map(&mut memory_set.page_table);

        if file_size > 0 {
            let data = &elf_data[offset..offset + file_size];
            memory_set.copy_to_user(va_start, data);
        }

        memory_set.areas.push(area);
    }

    // 计算brk起始（程序段结束后）
    let brk_start = (max_va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let brk_start = brk_start + PAGE_SIZE; // guard page

    // 如果有动态链接器，加载它
    let entry = if let Some(interp_path) = interp_path {
        load_interpreter(&interp_path, &mut memory_set)
    } else {
        elf_entry
    };

    // 分配用户栈
    let stack_bottom = brk_start + 4 * 1024 * 1024; // 在堆之后
    let stack_top = stack_bottom + USER_STACK_SIZE;

    memory_set.insert_framed_area(
        stack_bottom,
        stack_top,
        MapPerm::R | MapPerm::W | MapPerm::U,
    );

    // 分配初始堆（4MB）
    memory_set.insert_framed_area(
        brk_start,
        brk_start + 4 * 1024 * 1024,
        MapPerm::R | MapPerm::W | MapPerm::U,
    );

    // 构建用户栈（设置argv, envp, auxv）
    let user_sp = setup_stack(&mut memory_set, stack_top, phdr_va, phnum, elf_entry, load_base);

    ElfLoad {
        memory_set,
        entry,
        user_sp,
        phdr_va,
        phnum,
        elf_entry,
        elf_base: load_base,
        brk_start,
    }
}

fn load_interpreter(interp_path: &str, memory_set: &mut MemorySet) -> usize {
    // 从文件系统加载动态链接器
    let elf_data = match crate::fs::FS.lookup(interp_path) {
        Some(node) => {
            let node = node.lock();
            match &node.kind {
                crate::fs::ramfs::INodeKind::File(data) => {
                    let d = data.lock();
                    d.clone()
                }
                _ => {
                    println!("[elf] Interpreter not a file: {}", interp_path);
                    return 0;
                }
            }
        }
        None => {
            // 尝试备用路径
            let alt_path = if interp_path.contains("riscv64-linux-gnu") {
                interp_path.replace("/lib/", "/lib/riscv64-linux-gnu/")
            } else {
                interp_path.to_string()
            };
            match crate::fs::FS.lookup(&alt_path) {
                Some(node) => {
                    let node = node.lock();
                    match &node.kind {
                        crate::fs::ramfs::INodeKind::File(data) => data.lock().clone(),
                        _ => return 0,
                    }
                }
                None => {
                    println!("[elf] Interpreter not found: {}", interp_path);
                    return 0;
                }
            }
        }
    };

    let elf = match ElfFile::new(&elf_data) {
        Ok(e) => e,
        Err(e) => {
            println!("[elf] Failed to parse interpreter: {}", e);
            return 0;
        }
    };

    let entry_offset = elf.header.pt2.entry_point() as usize;

    // 加载interpreter的程序段（到INTERP_BASE）
    for ph in elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load { continue; }

        let va_start = ph.virtual_addr() as usize + INTERP_BASE;
        let va_end = va_start + ph.mem_size() as usize;
        let file_size = ph.file_size() as usize;
        let offset = ph.offset() as usize;

        let flags = ph.flags();
        let mut perm = MapPerm::U;
        if flags.is_read() { perm |= MapPerm::R; }
        if flags.is_write() { perm |= MapPerm::W; }
        if flags.is_execute() { perm |= MapPerm::X; }

        let va_start_aligned = va_start & !(PAGE_SIZE - 1);
        let va_end_aligned = (va_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let mut area = MapArea::new(va_start_aligned, va_end_aligned, MapType::Framed, perm);
        area.map(&mut memory_set.page_table);

        if file_size > 0 {
            let data_slice = &elf_data[offset..offset + file_size];
            memory_set.copy_to_user(va_start, data_slice);
        }

        memory_set.areas.push(area);
    }

    println!("[elf] Interpreter loaded at base {:#x}, entry {:#x}",
        INTERP_BASE, INTERP_BASE + entry_offset);
    INTERP_BASE + entry_offset
}

fn setup_stack(
    memory_set: &mut MemorySet,
    stack_top: usize,
    phdr_va: usize,
    phnum: usize,
    elf_entry: usize,
    load_base: usize,
) -> usize {
    // 在栈上设置POSIX初始栈帧:
    // argc, argv[], NULL, envp[], NULL, auxv[], NULL_PAIR
    // 然后是字符串数据

    let mut sp = stack_top;

    let write_usize = |sp: &mut usize, val: usize, ms: &mut MemorySet| {
        *sp -= 8;
        ms.copy_to_user(*sp, &val.to_le_bytes());
    };

    let write_str = |sp: &mut usize, s: &[u8], ms: &mut MemorySet| -> usize {
        *sp -= s.len() + 1;
        ms.copy_to_user(*sp, s);
        ms.copy_to_user(*sp + s.len(), &[0u8]);
        *sp
    };

    // 写入环境变量字符串
    let env_strings: &[&[u8]] = &[
        b"HOME=/tmp",
        b"PATH=/usr/sbin:/usr/bin:/sbin:/bin",
        b"TERM=vt100",
    ];
    let mut env_ptrs: Vec<usize> = Vec::new();
    for &s in env_strings.iter().rev() {
        env_ptrs.push(write_str(&mut sp, s, memory_set));
    }
    env_ptrs.reverse();

    // 写入argv字符串
    let argv_strings: &[&[u8]] = &[
        b"nginx",
        b"-g",
        b"daemon off;",
    ];
    let mut arg_ptrs: Vec<usize> = Vec::new();
    for &s in argv_strings.iter().rev() {
        arg_ptrs.push(write_str(&mut sp, s, memory_set));
    }
    arg_ptrs.reverse();

    // 对齐到16字节
    sp &= !15;

    // Auxiliary vector (AT_* values)
    // AT_NULL = 0
    write_usize(&mut sp, 0, memory_set); // value
    write_usize(&mut sp, 0, memory_set); // AT_NULL

    // AT_RANDOM = 25（16字节随机数的地址）
    // 先在栈上写16字节随机数
    sp -= 16;
    let random_addr = sp;
    let random_bytes: [u8; 16] = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                                   0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    memory_set.copy_to_user(random_addr, &random_bytes);
    write_usize(&mut sp, random_addr, memory_set);
    write_usize(&mut sp, 25, memory_set); // AT_RANDOM

    // AT_PAGESZ = 6
    write_usize(&mut sp, PAGE_SIZE, memory_set);
    write_usize(&mut sp, 6, memory_set);

    // AT_PHNUM = 5
    write_usize(&mut sp, phnum, memory_set);
    write_usize(&mut sp, 5, memory_set);

    // AT_PHENT = 4 (size of program header entry)
    write_usize(&mut sp, 56, memory_set); // sizeof(Elf64_Phdr)
    write_usize(&mut sp, 4, memory_set);

    // AT_PHDR = 3
    write_usize(&mut sp, phdr_va, memory_set);
    write_usize(&mut sp, 3, memory_set);

    // AT_ENTRY = 9 (original entry point)
    write_usize(&mut sp, elf_entry, memory_set);
    write_usize(&mut sp, 9, memory_set);

    // AT_FLAGS = 8
    write_usize(&mut sp, 0, memory_set);
    write_usize(&mut sp, 8, memory_set);

    // AT_BASE = 7 (base address of interpreter)
    write_usize(&mut sp, INTERP_BASE, memory_set);
    write_usize(&mut sp, 7, memory_set);

    // AT_UID = 11, AT_EUID = 12, AT_GID = 13, AT_EGID = 14
    write_usize(&mut sp, 0, memory_set); write_usize(&mut sp, 14, memory_set);
    write_usize(&mut sp, 0, memory_set); write_usize(&mut sp, 13, memory_set);
    write_usize(&mut sp, 0, memory_set); write_usize(&mut sp, 12, memory_set);
    write_usize(&mut sp, 0, memory_set); write_usize(&mut sp, 11, memory_set);

    // AT_SECURE = 23
    write_usize(&mut sp, 0, memory_set);
    write_usize(&mut sp, 23, memory_set);

    // AT_HWCAP = 16 (riscv capabilities)
    write_usize(&mut sp, 0x1190, memory_set); // IMACFD
    write_usize(&mut sp, 16, memory_set);

    // AT_CLKTCK = 17
    write_usize(&mut sp, 100, memory_set);
    write_usize(&mut sp, 17, memory_set);

    // NULL terminator for envp
    write_usize(&mut sp, 0, memory_set);

    // envp pointers
    for &ptr in env_ptrs.iter().rev() {
        write_usize(&mut sp, ptr, memory_set);
    }

    // NULL terminator for argv
    write_usize(&mut sp, 0, memory_set);

    // argv pointers
    for &ptr in arg_ptrs.iter().rev() {
        write_usize(&mut sp, ptr, memory_set);
    }

    // argc
    write_usize(&mut sp, arg_ptrs.len(), memory_set);

    sp
}

/// 公开版本：将内核映射到用户进程的页表中（供其他模块使用）
pub fn map_kernel_into_public(memory_set: &mut MemorySet) {
    map_kernel_into(memory_set);
}

/// 将内核映射到用户进程的页表中
fn map_kernel_into(memory_set: &mut MemorySet) {
    let kernel_satp = {
        crate::mm::KERNEL_SPACE.lock().token()
    };
    let kernel_root_ppn = kernel_satp & ((1 << 44) - 1);
    let kernel_root_pa = kernel_root_ppn << crate::config::PAGE_SIZE_BITS;
    let kernel_root = unsafe {
        core::slice::from_raw_parts(
            kernel_root_pa as *const u64,
            512,
        )
    };

    let user_root_ppn = memory_set.page_table.root_ppn().ppn();
    let user_root_pa = user_root_ppn << crate::config::PAGE_SIZE_BITS;
    let user_root = unsafe {
        core::slice::from_raw_parts_mut(
            user_root_pa as *mut u64,
            512,
        )
    };

    // VPN2=0: device IO, VPN2=2: kernel memory - share from KERNEL_SPACE
    user_root[0] = kernel_root[0];
    user_root[2] = kernel_root[2];
}
