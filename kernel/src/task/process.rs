//! Processes: address space, ELF exec, fork, page faults and mmap.

use crate::errno::*;
use crate::fs::file::*;
use crate::fs::{self, Inode, Kind};
use crate::mm::address::*;
use crate::mm::frame;
use crate::mm::page_table::*;
use crate::task::elf;
use crate::task::signal::SignalState;
use crate::trap::TrapFrame;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const USER_STACK_TOP: usize = 0x0000_0000_7fff_0000;
pub const USER_STACK_MAX: usize = 16 * 1024 * 1024;
pub const USER_MMAP_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_MMAP_LIMIT: usize = 0x0000_0000_7000_0000;
pub const PIE_BASE: usize = 0x0000_0000_0100_0000;
pub const INTERP_BASE: usize = 0x0000_0000_0400_0000;
pub const SIGRETURN_TRAMPOLINE: usize = 0x0000_0000_7e00_0000;

pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

pub const MAP_SHARED: u32 = 1;
pub const MAP_PRIVATE: u32 = 2;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;
pub const MAP_GROWSDOWN: u32 = 0x100;
pub const MAP_NORESERVE: u32 = 0x4000;
pub const MAP_POPULATE: u32 = 0x8000;
pub const MAP_STACK: u32 = 0x20000;
pub const MAP_FIXED_NOREPLACE: u32 = 0x100000;

#[derive(Clone)]
pub struct Area {
    pub start: usize,
    pub end: usize,
    pub prot: u32,
    pub flags: u32,
    pub file: Option<(Arc<Inode>, u64)>,
    pub growdown: bool,
}

impl Area {
    pub fn pte_flags(&self) -> usize {
        let mut f = PTE_U | PTE_A | PTE_D;
        if self.prot & PROT_READ != 0 {
            f |= PTE_R;
        }
        if self.prot & PROT_WRITE != 0 {
            f |= PTE_W;
        }
        if self.prot & PROT_EXEC != 0 {
            f |= PTE_X;
        }
        if f & PTE_W != 0 {
            f |= PTE_R;
        }
        f
    }
}

pub struct MemoryMap {
    pub areas: Vec<Area>,
    pub brk_start: usize,
    pub brk: usize,
    pub mmap_next: usize,
    pub stack_top: usize,
}

impl MemoryMap {
    pub fn new() -> Self {
        Self {
            areas: Vec::new(),
            brk_start: 0,
            brk: 0,
            mmap_next: USER_MMAP_BASE,
            stack_top: USER_STACK_TOP,
        }
    }

    pub fn find(&self, va: usize) -> Option<&Area> {
        self.areas.iter().find(|a| va >= a.start && va < a.end)
    }

    pub fn find_mut(&mut self, va: usize) -> Option<&mut Area> {
        self.areas.iter_mut().find(|a| va >= a.start && va < a.end)
    }

    pub fn add(
        &mut self,
        start: usize,
        end: usize,
        prot: u32,
        flags: u32,
        file: Option<(Arc<Inode>, u64)>,
        growdown: bool,
    ) {
        if end <= start {
            return;
        }
        self.areas.push(Area {
            start,
            end,
            prot,
            flags,
            file,
            growdown,
        });
        self.areas.sort_by_key(|a| a.start);
    }

    /// Apply a new protection to exactly [start, end), splitting areas as needed.
    pub fn set_prot_range(&mut self, start: usize, end: usize, prot: u32) {
        let mut out: Vec<Area> = Vec::new();
        for a in self.areas.drain(..) {
            if a.end <= start || a.start >= end {
                out.push(a);
                continue;
            }
            if a.start < start {
                let mut lo = a.clone();
                lo.end = start;
                out.push(lo);
            }
            let mut mid = a.clone();
            mid.start = a.start.max(start);
            mid.end = a.end.min(end);
            mid.prot = prot;
            out.push(mid);
            if a.end > end {
                let mut hi = a.clone();
                let delta = end - a.start;
                hi.start = end;
                if let Some((f, off)) = &hi.file {
                    hi.file = Some((f.clone(), off + delta as u64));
                }
                out.push(hi);
            }
        }
        self.areas = out;
        self.areas.sort_by_key(|a| a.start);
    }

    pub fn remove_range(&mut self, start: usize, end: usize) {
        let mut out: Vec<Area> = Vec::new();
        for a in self.areas.drain(..) {
            if a.end <= start || a.start >= end {
                out.push(a);
                continue;
            }
            if a.start < start {
                let mut lo = a.clone();
                lo.end = start;
                out.push(lo);
            }
            if a.end > end {
                let mut hi = a.clone();
                let delta = end - a.start;
                hi.start = end;
                if let Some((f, off)) = &hi.file {
                    hi.file = Some((f.clone(), off + delta as u64));
                }
                out.push(hi);
            }
        }
        self.areas = out;
    }

    pub fn is_free(&self, start: usize, end: usize) -> bool {
        !self.areas.iter().any(|a| a.start < end && a.end > start)
    }

    pub fn alloc_range(&mut self, len: usize, align: usize) -> Option<usize> {
        let len = page_align_up(len);
        let mut addr = page_align_up(self.mmap_next.max(USER_MMAP_BASE));
        if align > PAGE_SIZE {
            addr = (addr + align - 1) & !(align - 1);
        }
        while addr + len <= USER_MMAP_LIMIT {
            if self.is_free(addr, addr + len) {
                self.mmap_next = addr + len;
                return Some(addr);
            }
            addr += PAGE_SIZE;
        }
        None
    }
}

/// Resolve a fault: returns true if the page was made present.
pub fn fault_in(pt: &mut PageTable, mm: &mut MemoryMap, va: usize, write: bool) -> bool {
    let area = match mm.find(va) {
        Some(a) => a.clone(),
        None => return false,
    };
    if write && area.prot & PROT_WRITE == 0 {
        return false;
    }
    let page = page_align_down(va);
    if pt.translate_user(page).is_some() {
        return true;
    }
    let frame = match frame::alloc_frame() {
        Some(f) => f,
        None => return false,
    };
    if let Some((inode, file_off)) = &area.file {
        let off = file_off + (page - area.start) as u64;
        let size = inode.size();
        if off < size {
            let n = core::cmp::min(PAGE_SIZE as u64, size - off) as usize;
            let dst = unsafe { core::slice::from_raw_parts_mut(frame as *mut u8, n) };
            let _ = inode.read_at(off, dst);
        }
    }
    if pt.map(page, frame, area.pte_flags()).is_err() {
        frame::free_frame(frame);
        return false;
    }
    true
}

pub struct Process {
    pub pid: usize,
    pub ppid: usize,
    pub pgid: usize,
    pub sid: usize,
    pub pt: PageTable,
    pub mm: MemoryMap,
    pub files: FdTable,
    pub cwd: Arc<Inode>,
    pub exit_code: i32,
    pub exited: bool,
    pub sig: SignalState,
    pub comm: String,
    pub exe: String,
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub umask: u32,
    pub start_time: u64,
    pub children: Vec<usize>,
    pub clear_child_tid: usize,
    pub robust_list: usize,
    pub robust_list_len: usize,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub init_tf: TrapFrame,
    pub exec_path: String,
    pub did_exec: bool,
    pub wait_status: i32,
    pub parent_waiting: bool,
    pub kill_sig: i32,
    pub sig_frame_sp: usize,
}

impl Process {
    pub fn new(pid: usize) -> Self {
        let root = fs::root();
        let pt = PageTable::new_user(crate::task::KERNEL_ROOT.load(core::sync::atomic::Ordering::Relaxed));
        Self {
            pid,
            ppid: 0,
            pgid: pid,
            sid: pid,
            pt,
            mm: MemoryMap::new(),
            files: FdTable::new(),
            cwd: root,
            exit_code: 0,
            exited: false,
            sig: SignalState::new(),
            comm: String::new(),
            exe: String::new(),
            argv: Vec::new(),
            envp: Vec::new(),
            umask: 0o022,
            start_time: crate::time::uptime_ns(),
            children: Vec::new(),
            clear_child_tid: 0,
            robust_list: 0,
            robust_list_len: 0,
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            init_tf: TrapFrame::zeroed(),
            exec_path: String::new(),
            did_exec: false,
            wait_status: 0,
            parent_waiting: false,
            kill_sig: 0,
            sig_frame_sp: 0,
        }
    }

    pub fn satp(&self) -> usize {
        self.pt.satp()
    }

    pub fn activate(&self) {
        crate::csr::set_satp(self.satp());
    }

    pub fn free_memory(&mut self) {
        for a in self.mm.areas.iter() {
            let mut va = page_align_down(a.start);
            while va < a.end {
                if let Some(pa) = self.pt.translate_user(va) {
                    self.pt.unmap(va);
                    frame::free_frame(pa);
                }
                va += PAGE_SIZE;
            }
        }
        self.pt.free_tables();
        self.mm.areas.clear();
    }

    pub fn copy_to_user(&mut self, va: usize, data: &[u8]) -> Result<(), Errno> {
        let mut off = 0;
        while off < data.len() {
            let addr = va + off;
            let page = page_align_down(addr);
            let pa = match self.pt.translate_user(page) {
                Some(pa) => pa,
                None => {
                    if !fault_in(&mut self.pt, &mut self.mm, page, true) {
                        return Err(EFAULT);
                    }
                    self.pt.translate_user(page).ok_or(EFAULT)?
                }
            };
            let inner = addr - page;
            let n = core::cmp::min(PAGE_SIZE - inner, data.len() - off);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(off),
                    (pa + inner) as *mut u8,
                    n,
                );
            }
            off += n;
        }
        Ok(())
    }

    pub fn copy_from_user(&mut self, va: usize, out: &mut [u8]) -> Result<(), Errno> {
        let mut off = 0;
        while off < out.len() {
            let addr = va + off;
            let page = page_align_down(addr);
            let pa = match self.pt.translate_user(page) {
                Some(pa) => pa,
                None => {
                    if !fault_in(&mut self.pt, &mut self.mm, page, false) {
                        return Err(EFAULT);
                    }
                    self.pt.translate_user(page).ok_or(EFAULT)?
                }
            };
            let inner = addr - page;
            let n = core::cmp::min(PAGE_SIZE - inner, out.len() - off);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (pa + inner) as *const u8,
                    out.as_mut_ptr().add(off),
                    n,
                );
            }
            off += n;
        }
        Ok(())
    }

    pub fn read_cstr(&mut self, va: usize, max: usize) -> Result<String, Errno> {
        if va == 0 {
            return Err(EFAULT);
        }
        let mut out = Vec::new();
        let mut addr = va;
        while out.len() < max {
            let page = page_align_down(addr);
            let pa = match self.pt.translate_user(page) {
                Some(pa) => pa,
                None => {
                    if !fault_in(&mut self.pt, &mut self.mm, page, false) {
                        return Err(EFAULT);
                    }
                    self.pt.translate_user(page).ok_or(EFAULT)?
                }
            };
            let inner = addr - page;
            let n = core::cmp::min(PAGE_SIZE - inner, max - out.len());
            let src = unsafe { core::slice::from_raw_parts((pa + inner) as *const u8, n) };
            if let Some(pos) = src.iter().position(|&b| b == 0) {
                out.extend_from_slice(&src[..pos]);
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            out.extend_from_slice(src);
            addr += n;
        }
        Err(ENAMETOOLONG)
    }

    pub fn mmap(
        &mut self,
        addr: usize,
        len: usize,
        prot: u32,
        flags: u32,
        file: Option<(Arc<Inode>, u64)>,
    ) -> Result<usize, Errno> {
        if len == 0 {
            return Err(EINVAL);
        }
        let len = page_align_up(len);
        let start = if flags & MAP_FIXED != 0 {
            if addr & (PAGE_SIZE - 1) != 0 {
                return Err(EINVAL);
            }
            addr
        } else if addr != 0 && (addr & (PAGE_SIZE - 1)) == 0 && self.mm.is_free(addr, addr + len) {
            addr
        } else {
            self.mm.alloc_range(len, PAGE_SIZE).ok_or(ENOMEM)?
        };
        if flags & MAP_FIXED != 0 {
            let mut va = start;
            while va < start + len {
                if let Some(pa) = self.pt.translate_user(va) {
                    self.pt.unmap(va);
                    frame::free_frame(pa);
                }
                va += PAGE_SIZE;
            }
            self.mm.remove_range(start, start + len);
        } else if !self.mm.is_free(start, start + len) {
            return Err(ENOMEM);
        }
        self.mm.add(start, start + len, prot, flags, file, flags & MAP_GROWSDOWN != 0);
        if flags & MAP_POPULATE != 0 {
            let mut va = start;
            while va < start + len {
                fault_in(&mut self.pt, &mut self.mm, va, false);
                va += PAGE_SIZE;
            }
        }
        Ok(start)
    }

    pub fn munmap(&mut self, addr: usize, len: usize) -> Result<(), Errno> {
        if addr & (PAGE_SIZE - 1) != 0 {
            return Err(EINVAL);
        }
        let end = page_align_up(addr + len);
        let mut va = page_align_down(addr);
        while va < end {
            if let Some(pa) = self.pt.translate_user(va) {
                if let Some(a) = self.mm.find(va).cloned() {
                    if a.flags & MAP_SHARED != 0 {
                        if let Some((inode, foff)) = &a.file {
                            let off = foff + (va - a.start) as u64;
                            let data =
                                unsafe { core::slice::from_raw_parts(pa as *const u8, PAGE_SIZE) };
                            let _ = inode.write_at(off, data);
                        }
                    }
                }
                self.pt.unmap(va);
                frame::free_frame(pa);
            }
            va += PAGE_SIZE;
        }
        self.mm.remove_range(page_align_down(addr), end);
        Ok(())
    }

    pub fn mprotect(&mut self, addr: usize, len: usize, prot: u32) -> Result<(), Errno> {
        let start = page_align_down(addr);
        let end = page_align_up(addr + len);
        self.mm.set_prot_range(start, end, prot);
        let flags = Area {
            start,
            end,
            prot,
            flags: 0,
            file: None,
            growdown: false,
        }
        .pte_flags();
        let mut va = start;
        while va < end {
            if self.pt.translate_user(va).is_some() {
                self.pt.set_flags(va, flags);
            }
            va += PAGE_SIZE;
        }
        Ok(())
    }

    pub fn brk(&mut self, new: usize) -> usize {
        if new == 0 {
            return self.mm.brk;
        }
        if new < self.mm.brk_start {
            return self.mm.brk;
        }
        if new > self.mm.brk {
            let start = page_align_up(self.mm.brk);
            let end = page_align_up(new);
            if end > start && !self.mm.is_free(start, end) {
                return self.mm.brk;
            }
            self.mm.add(
                page_align_down(self.mm.brk),
                end,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                None,
                false,
            );
        } else {
            let start = page_align_up(new);
            let end = page_align_up(self.mm.brk);
            let mut va = start;
            while va < end {
                if let Some(pa) = self.pt.translate_user(va) {
                    self.pt.unmap(va);
                    frame::free_frame(pa);
                }
                va += PAGE_SIZE;
            }
            self.mm.remove_range(start, end);
            self.mm.add(
                page_align_down(new),
                page_align_up(new),
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                None,
                false,
            );
        }
        self.mm.brk = new;
        new
    }

    pub fn exec(&mut self, path: &str, argv: Vec<String>, envp: Vec<String>) -> Result<(), Errno> {
        let inode = fs::lookup(&self.cwd, path, true)?;
        if inode.kind() != Kind::File {
            return Err(ENOEXEC);
        }
        let size = inode.size() as usize;
        let mut data = alloc::vec![0u8; size];
        inode.read_at(0, &mut data)?;
        let img = elf::parse(&data).map_err(|_| ENOEXEC)?;

        let mut pt = PageTable::new_user(crate::task::KERNEL_ROOT.load(core::sync::atomic::Ordering::Relaxed));
        let mut mm = MemoryMap::new();

        let main_base = if img.is_dyn { PIE_BASE } else { 0 };
        let phoff = elf::phoff(&data);
        let phs = elf::phdrs(&data, phoff, img.phnum, img.phent);
        let mut load_end = 0usize;
        for ph in phs.iter() {
            if ph.p_type == elf::PT_LOAD {
                let end = elf::load_segment(&mut pt, &data, ph, main_base).map_err(|_| ENOMEM)?;
                load_end = load_end.max(end);
            }
        }
        let main_start = if img.is_dyn { PIE_BASE } else { 0x10000 };
        mm.add(
            main_start,
            load_end.max(main_start + PAGE_SIZE),
            PROT_READ | PROT_WRITE | PROT_EXEC,
            MAP_PRIVATE,
            None,
            false,
        );

        let mut entry = img.entry + main_base;
        let mut at_base = 0usize;
        let phdr_addr;

        if let Some(interp) = &img.interp {
            let iinode = fs::lookup(&self.cwd, interp, true)?;
            let isize = iinode.size() as usize;
            let mut idata = alloc::vec![0u8; isize];
            iinode.read_at(0, &mut idata)?;
            let iimg = elf::parse(&idata).map_err(|_| ENOEXEC)?;
            let iphoff = elf::phoff(&idata);
            let iphs = elf::phdrs(&idata, iphoff, iimg.phnum, iimg.phent);
            let mut iend = 0usize;
            for ph in iphs.iter() {
                if ph.p_type == elf::PT_LOAD {
                    let end =
                        elf::load_segment(&mut pt, &idata, ph, INTERP_BASE).map_err(|_| ENOMEM)?;
                    iend = iend.max(end);
                }
            }
            mm.add(
                INTERP_BASE,
                iend,
                PROT_READ | PROT_WRITE | PROT_EXEC,
                MAP_PRIVATE,
                None,
                false,
            );
            at_base = INTERP_BASE;
            entry = iimg.entry + INTERP_BASE;
        }
        phdr_addr = main_base + phoff;

        // sigreturn trampoline
        let tramp = frame::alloc_frame().ok_or(ENOMEM)?;
        unsafe {
            let code = tramp as *mut u32;
            code.write(0x08b0_0893); // li a7, 139 (rt_sigreturn)
            code.add(1).write(0x0000_0073); // ecall
            code.add(2).write(0x0000_006f); // j .
        }
        pt.map(
            SIGRETURN_TRAMPOLINE,
            tramp,
            PTE_U | PTE_R | PTE_X | PTE_A | PTE_D,
        )
        .map_err(|_| ENOMEM)?;
        mm.add(
            SIGRETURN_TRAMPOLINE,
            SIGRETURN_TRAMPOLINE + PAGE_SIZE,
            PROT_READ | PROT_EXEC,
            MAP_PRIVATE,
            None,
            false,
        );

        // heap
        let brk_start = page_align_up(load_end.max(main_start + PAGE_SIZE));
        mm.brk_start = brk_start;
        mm.brk = brk_start;

        // user stack
        mm.stack_top = USER_STACK_TOP;
        mm.add(
            USER_STACK_TOP - USER_STACK_MAX,
            USER_STACK_TOP,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            None,
            true,
        );

        self.free_memory();
        self.pt = pt;
        self.mm = mm;
        self.activate();

        let sp = self.setup_stack(
            &argv,
            &envp,
            &img,
            main_base,
            at_base,
            entry,
            path,
            phdr_addr,
        )?;

        self.argv = argv;
        self.envp = envp;
        self.exec_path = path.to_string();
        self.comm = path.rsplit('/').next().unwrap_or(path).to_string();
        self.exe = path.to_string();
        self.did_exec = true;
        self.sig.reset_for_exec();
        self.init_tf = TrapFrame::zeroed();
        self.init_tf.sepc = entry;
        self.init_tf.x[2] = sp;
        self.init_tf.x[10] = 0;
        self.init_tf.sstatus = crate::csr::SSTATUS_SPIE | (3 << 13);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_stack(
        &mut self,
        argv: &[String],
        envp: &[String],
        img: &elf::ElfImage,
        main_base: usize,
        at_base: usize,
        _entry: usize,
        path: &str,
        phdr_addr: usize,
    ) -> Result<usize, Errno> {
        let mut sp = USER_STACK_TOP - 128;
        let mut writes: Vec<(usize, Vec<u8>)> = Vec::new();

        let mut push_bytes = |sp: &mut usize, b: &[u8]| -> usize {
            *sp -= b.len() + 1;
            *sp &= !7;
            writes.push((*sp, {
                let mut v = b.to_vec();
                v.push(0);
                v
            }));
            *sp
        };

        let mut arg_ptrs = Vec::new();
        for a in argv.iter() {
            let p = push_bytes(&mut sp, a.as_bytes());
            arg_ptrs.push(p);
        }
        let mut env_ptrs = Vec::new();
        for e in envp.iter() {
            let p = push_bytes(&mut sp, e.as_bytes());
            env_ptrs.push(p);
        }
        let execfn = push_bytes(&mut sp, path.as_bytes());
        let platform_ptr = push_bytes(&mut sp, b"riscv64");
        sp -= 16;
        let random_ptr = sp;
        let mut rnd = [0u8; 16];
        let t = crate::time::rdtime();
        for (i, b) in rnd.iter_mut().enumerate() {
            *b = ((t >> ((i % 8) * 8)) as u8) ^ (i as u8).wrapping_mul(31);
        }
        writes.push((random_ptr, rnd.to_vec()));

        let auxv: Vec<(usize, usize)> = alloc::vec![
            (3, phdr_addr),
            (4, img.phent),
            (5, img.phnum),
            (6, PAGE_SIZE),
            (7, at_base),
            (8, 0),
            (9, img.entry + main_base),
            (11, self.uid as usize),
            (12, self.euid as usize),
            (13, self.gid as usize),
            (14, self.egid as usize),
            (15, platform_ptr),
            (16, 0),
            (17, 100),
            (23, 0),
            (25, random_ptr),
            (26, 0),
            (31, execfn),
            (33, 0),
            (51, 8192),
            (0, 0),
        ];

        let nwords = 1 + arg_ptrs.len() + 1 + env_ptrs.len() + 1 + auxv.len() * 2;
        sp = (sp - nwords * 8) & !15;
        let base = sp;
        let mut w = base;
        let mut put = |w: &mut usize, v: usize| {
            writes.push((*w, v.to_ne_bytes().to_vec()));
            *w += 8;
        };
        put(&mut w, argv.len());
        for p in arg_ptrs.iter() {
            put(&mut w, *p);
        }
        put(&mut w, 0);
        for p in env_ptrs.iter() {
            put(&mut w, *p);
        }
        put(&mut w, 0);
        for (k, v) in auxv.iter() {
            put(&mut w, *k);
            put(&mut w, *v);
        }

        for (addr, bytes) in writes.iter() {
            self.copy_to_user(*addr, bytes)?;
        }
        Ok(base)
    }

    pub fn fork_from(parent: &mut Process, child: &mut Process) -> Result<(), Errno> {
        child.mm = MemoryMap {
            areas: parent.mm.areas.clone(),
            brk_start: parent.mm.brk_start,
            brk: parent.mm.brk,
            mmap_next: parent.mm.mmap_next,
            stack_top: parent.mm.stack_top,
        };
        child.files = parent.files.clone_table();
        child.cwd = parent.cwd.clone();
        child.comm = parent.comm.clone();
        child.exe = parent.exe.clone();
        child.argv = parent.argv.clone();
        child.envp = parent.envp.clone();
        child.umask = parent.umask;
        child.sig = parent.sig.clone();
        child.clear_child_tid = parent.clear_child_tid;
        child.robust_list = parent.robust_list;
        child.robust_list_len = parent.robust_list_len;
        child.init_tf = parent.init_tf;
        child.exec_path = parent.exec_path.clone();
        child.uid = parent.uid;
        child.gid = parent.gid;
        child.euid = parent.euid;
        child.egid = parent.egid;
        child.ppid = parent.pid;
        child.pgid = parent.pgid;
        child.sid = parent.sid;
        for a in parent.mm.areas.iter() {
            let mut va = page_align_down(a.start);
            while va < a.end {
                if child.pt.translate_user(va).is_some() {
                    va += PAGE_SIZE;
                    continue;
                }
                if let Some((pa, pte)) = parent.pt.translate(va) {
                    if pte & PTE_U == 0 {
                        va += PAGE_SIZE;
                        continue;
                    }
                    let nf = frame::alloc_frame().ok_or(ENOMEM)?;
                    unsafe {
                        core::ptr::copy_nonoverlapping(pa as *const u8, nf as *mut u8, PAGE_SIZE);
                    }
                    let flags = (pte & 0xff) | PTE_A | PTE_D;
                    child.pt.map(va, nf, flags).map_err(|_| ENOMEM)?;
                }
                va += PAGE_SIZE;
            }
        }
        Ok(())
    }

    pub fn init_stdio(&mut self) {
        for fd in 0..3 {
            let f = File::new(
                FileObj::CharDev {
                    rdev: crate::fs::chardev::DEV_CONSOLE,
                },
                0,
                false,
            );
            let _ = self.files.set(fd, f);
        }
    }
}

pub fn spawn_init() -> Result<usize, &'static str> {
    let bootargs = crate::BOOTARGS.load();
    let mut path = "/usr/sbin/nginx".to_string();
    for tok in bootargs.split_whitespace() {
        if let Some(v) = tok.strip_prefix("init=") {
            path = v.to_string();
        }
    }
    let mut argv = Vec::new();
    argv.push(path.clone());
    if path.ends_with("nginx") {
        argv.push("-c".to_string());
        argv.push("/etc/nginx/nginx.conf".to_string());
    }
    let envp: Vec<String> = alloc::vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "HOME=/root".to_string(),
        "TERM=linux".to_string(),
        "USER=root".to_string(),
        "LANG=C".to_string(),
        "PWD=/".to_string(),
    ];

    let tid = crate::task::alloc_tid();
    let mut p = Process::new(tid);
    p.init_stdio();
    p.exec(&path, argv, envp).map_err(|_| "exec failed")?;
    crate::println!("[init] launching {} (pid {})", path, tid);
    Ok(crate::task::spawn_user(p))
}

pub fn handle_page_fault(_tf: &mut TrapFrame, va: usize, scause: usize) -> bool {
    let p = match crate::task::current().process.as_mut() {
        Some(p) => p,
        None => return false,
    };
    let write = scause == crate::csr::SCAUSE_STORE_PAGE_FAULT;
    if fault_in(&mut p.pt, &mut p.mm, va, write) {
        return true;
    }
    // stack growth below a growdown area
    let grow = p
        .mm
        .areas
        .iter()
        .find(|a| a.growdown && va < a.start && va + 0x20000 > a.start)
        .cloned();
    if let Some(a) = grow {
        let start = page_align_down(va);
        p.mm.remove_range(start, a.start);
        p.mm
            .add(start, a.end, a.prot, a.flags, a.file.clone(), true);
        if fault_in(&mut p.pt, &mut p.mm, va, write) {
            return true;
        }
    }
    false
}

pub fn on_thread_exit(_i: usize) {}

/// Wake a parent blocked in wait4 and deliver SIGCHLD when a child exits.
pub fn notify_parent(pid: usize, status: i32) {
    use crate::task::signal::{SigInfo, SIGCHLD};
    let s = crate::task::sched();
    for i in 0..s.threads.len() {
        let mut hit = false;
        if let Some(p) = s.threads[i].process.as_mut() {
            if p.children.contains(&pid) {
                p.wait_status = status;
                p.sig.raise_with(
                    SIGCHLD,
                    SigInfo {
                        code: 1, // CLD_EXITED
                        pid: pid as i32,
                        uid: p.uid,
                        status: (status & 0xff) << 8,
                        ..Default::default()
                    },
                );
                hit = true;
            }
        }
        if hit {
            if s.threads[i].state == crate::task::State::Blocked {
                s.threads[i].state = crate::task::State::Ready;
                s.ready.push_back(i);
            }
        }
    }
}
