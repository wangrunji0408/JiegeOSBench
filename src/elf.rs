//! ELF64 加载与用户初始栈构建（含 PT_INTERP 动态链接器）

use crate::config::{PAGE_SIZE, USER_STACK_SIZE, USER_STACK_TOP};
use crate::mm::{AddressSpace, MapPerm, VirtAddr};
use crate::task::{new_task, Task};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use xmas_elf::program::Type as PhType;
use xmas_elf::ElfFile;

/// 主程序 PIE 加载基址
pub const EXE_BASE: usize = 0x4000_0000;
/// 动态链接器加载基址
pub const INTERP_BASE: usize = 0x3e_0000_0000;

pub const AT_NULL: usize = 0;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_ENTRY: usize = 9;
pub const AT_UID: usize = 11;
pub const AT_EUID: usize = 12;
pub const AT_GID: usize = 13;
pub const AT_EGID: usize = 14;
pub const AT_HWCAP: usize = 16;
pub const AT_CLKTCK: usize = 17;
pub const AT_SECURE: usize = 23;
pub const AT_RANDOM: usize = 25;

struct LoadResult {
    entry: usize,
    brk: usize,
    phdr: usize,
    phent: usize,
    phnum: usize,
    interp: Option<String>,
}

fn perm_of(flags: xmas_elf::program::Flags) -> MapPerm {
    let mut p = MapPerm::U;
    if flags.is_read() {
        p |= MapPerm::R;
    }
    if flags.is_write() {
        p |= MapPerm::W;
    }
    if flags.is_execute() {
        p |= MapPerm::X;
    }
    p
}

/// 加载一个 ELF 到地址空间，bias 仅对 ET_DYN 生效
fn load_elf(space: &mut AddressSpace, data: &[u8], bias: usize) -> Result<LoadResult, &'static str> {
    let elf = ElfFile::new(data).map_err(|_| "bad elf")?;
    let header = &elf.header;
    if header.pt1.magic != [0x7f, b'E', b'L', b'F'] {
        return Err("bad magic");
    }
    if header.pt2.machine().as_machine() != xmas_elf::header::Machine::RISC_V {
        return Err("not riscv");
    }
    let is_dyn = header.pt2.type_().as_type() == xmas_elf::header::Type::SharedObject;
    let base = if is_dyn { bias } else { 0 };

    let mut max_end = 0usize;
    let mut interp = None;
    let mut phdr_va = 0usize;
    let phoff = header.pt2.ph_offset() as usize;
    let phent = header.pt2.ph_entry_size() as usize;
    let phnum = header.pt2.ph_count() as usize;

    for ph in elf.program_iter() {
        match ph.get_type().map_err(|_| "bad ph")? {
            PhType::Load => {
                let start = base + ph.virtual_addr() as usize;
                let end = start + ph.mem_size() as usize;
                let file_size = ph.file_size() as usize;
                let offset = ph.offset() as usize;
                let perm = perm_of(ph.flags());
                let area = crate::mm::MapArea::new(
                    VirtAddr(start),
                    VirtAddr(end),
                    perm,
                );
                // 拷贝文件数据（从段起始页内偏移处开始）
                let seg_data = &data[offset..offset + file_size];
                space.map_area(area, None);
                // 逐页写入数据
                let mut written = 0usize;
                while written < file_size {
                    let va = start + written;
                    let pa = space.translate(va).ok_or("seg not mapped")?;
                    let page_off = va & (PAGE_SIZE - 1);
                    let len = core::cmp::min(PAGE_SIZE - page_off, file_size - written);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            seg_data.as_ptr().add(written),
                            pa as *mut u8,
                            len,
                        );
                    }
                    written += len;
                }
                if end > max_end {
                    max_end = end;
                }
                // 检查 program header 是否在此段中
                if phdr_va == 0 && phoff >= offset && phoff < offset + file_size {
                    phdr_va = start + (phoff - offset);
                }
            }
            PhType::Interp => {
                let offset = ph.offset() as usize;
                let size = ph.file_size() as usize;
                let path = core::str::from_utf8(&data[offset..offset + size])
                    .map_err(|_| "bad interp")?
                    .trim_end_matches('\0');
                interp = Some(String::from(path));
            }
            _ => {}
        }
    }

    let brk = (max_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    Ok(LoadResult {
        entry: base + header.pt2.entry_point() as usize,
        brk,
        phdr: phdr_va,
        phent,
        phnum,
        interp,
    })
}

/// 构建用户初始栈，返回 sp
fn setup_user_stack(
    space: &mut AddressSpace,
    args: &[String],
    envs: &[String],
    auxv: &[(usize, usize)],
) -> usize {
    // 映射用户栈
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
    let area = crate::mm::MapArea::new(
        VirtAddr(stack_bottom),
        VirtAddr(USER_STACK_TOP),
        MapPerm::R | MapPerm::W | MapPerm::U,
    );
    space.map_area(area, None);

    let mut sp = USER_STACK_TOP;
    let mut push_bytes = |space: &AddressSpace, sp: &mut usize, data: &[u8]| {
        *sp -= data.len();
        crate::mm::copy_out(space, *sp, data).unwrap();
    };
    let mut push_usize = |space: &AddressSpace, sp: &mut usize, v: usize| {
        *sp -= 8;
        crate::mm::copy_out(space, *sp, &v.to_ne_bytes()).unwrap();
    };

    // 字符串（逆序压入）
    let mut arg_ptrs = Vec::new();
    for arg in args.iter().rev() {
        let mut s = arg.clone().into_bytes();
        s.push(0);
        push_bytes(space, &mut sp, &s);
        arg_ptrs.push(sp);
    }
    arg_ptrs.reverse();
    let mut env_ptrs = Vec::new();
    for env in envs.iter().rev() {
        let mut s = env.clone().into_bytes();
        s.push(0);
        push_bytes(space, &mut sp, &s);
        env_ptrs.push(sp);
    }
    env_ptrs.reverse();
    // AT_RANDOM 16 字节
    sp -= 16;
    let random_va = sp;
    let mut rand_bytes = [0u8; 16];
    let seed = crate::timer::get_time();
    let mut x = seed | 1;
    for b in rand_bytes.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    crate::mm::copy_out(space, sp, &rand_bytes).unwrap();
    // 对齐到 16
    sp &= !15;

    // auxv（逆序）
    push_usize(space, &mut sp, 0);
    push_usize(space, &mut sp, 0);
    let mut aux: Vec<(usize, usize)> = auxv.to_vec();
    aux.push((AT_RANDOM, random_va));
    aux.push((AT_PAGESZ, PAGE_SIZE));
    aux.push((AT_HWCAP, 0));
    aux.push((AT_CLKTCK, 100));
    aux.push((AT_UID, 0));
    aux.push((AT_EUID, 0));
    aux.push((AT_GID, 0));
    aux.push((AT_EGID, 0));
    aux.push((AT_SECURE, 0));
    for (k, v) in aux.iter().rev() {
        push_usize(space, &mut sp, *v);
        push_usize(space, &mut sp, *k);
    }
    // envp
    push_usize(space, &mut sp, 0);
    for p in env_ptrs.iter().rev() {
        push_usize(space, &mut sp, *p);
    }
    // argv
    push_usize(space, &mut sp, 0);
    for p in arg_ptrs.iter().rev() {
        push_usize(space, &mut sp, *p);
    }
    // argc
    push_usize(space, &mut sp, args.len());
    sp
}

pub struct ExecImage {
    pub space: AddressSpace,
    pub entry: usize,
    pub sp: usize,
    pub brk: usize,
    pub name: String,
}

/// 构建执行镜像（地址空间 + 入口 + 初始栈）
pub fn build_image(
    elf_data: &[u8],
    args: Vec<String>,
    envs: Vec<String>,
) -> Result<ExecImage, &'static str> {
    let mut space = AddressSpace::new_user();
    let main = load_elf(&mut space, elf_data, EXE_BASE)?;

    let (entry, at_base) = if let Some(interp_path) = &main.interp {
        // 加载动态链接器
        let interp_data = crate::fs::with_fs(|fs| match fs.lookup(interp_path, "/", true) {
            Ok(id) => Ok(fs.nodes[id].data.clone()),
            Err(e) => Err(e),
        })
        .map_err(|e| {
            println!("interp lookup failed: path=[{}] err={}", interp_path, e);
            "interp not found"
        })?;
        let interp = load_elf(&mut space, &interp_data, INTERP_BASE)?;
        (interp.entry, INTERP_BASE)
    } else {
        (main.entry, 0)
    };

    let auxv = alloc::vec![
        (AT_PHDR, main.phdr),
        (AT_PHENT, main.phent),
        (AT_PHNUM, main.phnum),
        (AT_BASE, at_base),
        (AT_ENTRY, main.entry),
    ];
    let sp = setup_user_stack(&mut space, &args, &envs, &auxv);
    let name = args.get(0).cloned().unwrap_or_default();
    unsafe { core::arch::asm!("fence.i") };
    Ok(ExecImage {
        space,
        entry,
        sp,
        brk: main.brk,
        name,
    })
}

/// 从 ELF 数据创建新任务（用于 init 与 fork+exec）
pub fn exec_task(
    elf_data: &[u8],
    args: Vec<String>,
    envs: Vec<String>,
) -> Result<Arc<Task>, &'static str> {
    let img = build_image(elf_data, args, envs)?;
    let task = new_task(img.space, img.name);
    {
        let mut inner = task.inner.lock();
        inner.brk_start = img.brk;
        inner.brk = img.brk;
        // 标准输入输出
        inner.fd_table.push(Some(Arc::new(crate::fd::Fd::new(
            crate::fd::FdKind::Stdin,
        ))));
        inner.fd_table.push(Some(Arc::new(crate::fd::Fd::new(
            crate::fd::FdKind::Stdout,
        ))));
        inner.fd_table.push(Some(Arc::new(crate::fd::Fd::new(
            crate::fd::FdKind::Stderr,
        ))));
    }
    let cx = task.trap_cx();
    cx.sepc = img.entry;
    cx.x[2] = img.sp;
    Ok(task)
}
