//! Memory management syscalls.

use crate::fs::Result;
use crate::mm::{self, Backing, Prot};
use crate::{bail, task};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

/// `mmap` flags.
const MAP_SHARED: u32 = 0x01;
const MAP_PRIVATE: u32 = 0x02;
const MAP_FIXED: u32 = 0x10;
const MAP_ANONYMOUS: u32 = 0x20;
const MAP_NORESERVE: u32 = 0x4000;
const MAP_GROWSDOWN: u32 = 0x0100;
const MAP_STACK: u32 = 0x20000;
const MAP_POPULATE: u32 = 0x8000;
const MAP_FIXED_NOREPLACE: u32 = 0x100000;

/// `PROT_*`.
const PROT_NONE: u32 = 0;
const PROT_READ: u32 = 1;
const PROT_WRITE: u32 = 2;
const PROT_EXEC: u32 = 4;

fn to_prot(prot: u32) -> Prot {
    let mut p = Prot::empty();
    if prot & PROT_READ != 0 {
        p |= Prot::READ;
    }
    if prot & PROT_WRITE != 0 {
        p |= Prot::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        p |= Prot::EXEC;
    }
    p
}

pub fn sys_brk(new_brk: usize) -> Result<isize> {
    let task = task::current();
    let mut aspace = task.aspace.lock();

    if new_brk == 0 {
        return Ok(aspace.brk as isize);
    }
    // Refuse to move the break below where the heap started, or so far up that
    // it would run into the mmap region.
    if new_brk < aspace.brk_start || new_brk >= mm::USER_MMAP_BASE {
        return Ok(aspace.brk as isize);
    }

    let start = aspace.brk_start;
    // Resize the heap VMA in place. Using `map_region` here would unmap the old
    // range first and discard everything the program has allocated.
    if !aspace.resize_vma(start, mm::page_up(new_brk)) {
        // No heap VMA yet (or something is in the way): report the old break,
        // which tells malloc to fall back to mmap.
        return Ok(aspace.brk as isize);
    }
    aspace.brk = new_brk;
    Ok(new_brk as isize)
}

pub fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: usize,
) -> Result<isize> {
    if len == 0 {
        bail!(EINVAL);
    }
    if offset & mm::PAGE_MASK != 0 {
        bail!(EINVAL);
    }
    let len = mm::page_up(len);
    let task = task::current();

    let anonymous = flags & MAP_ANONYMOUS != 0;
    let shared = flags & MAP_SHARED != 0;
    let fixed = flags & MAP_FIXED != 0 || flags & MAP_FIXED_NOREPLACE != 0;

    // Resolve the backing.
    let (backing, name) = if anonymous {
        (Backing::Anon, if flags & MAP_STACK != 0 { "[stack]" } else { "[anon]" })
    } else {
        let file = task.files.lock().get_or_err(fd)?;
        if !file.readable() {
            bail!(EACCES);
        }
        // A shared writable file mapping would need writeback on unmap; nginx
        // only uses MAP_SHARED for anonymous zones, so private is enough here.
        (
            Backing::File {
                file: file.clone(),
                offset,
                // A plain `mmap` reads the whole file; the kernel already stops
                // at EOF, so the limit only has to not truncate anything.
                limit: usize::MAX,
            },
            "[file]",
        )
    };

    let mut aspace = task.aspace.lock();

    let start = if fixed {
        if addr == 0 || !mm::is_page_aligned(addr) {
            bail!(EINVAL);
        }
        if !mm::is_user_addr(addr) || addr + len > mm::USER_TOP {
            bail!(EINVAL);
        }
        if flags & MAP_FIXED_NOREPLACE != 0 && aspace.find_vma(addr).is_some() {
            bail!(EEXIST);
        }
        addr
    } else if addr != 0 && mm::is_user_addr(addr) && aspace.find_vma(addr).is_none() && addr + len <= mm::USER_TOP {
        // A hint we can honor.
        addr
    } else {
        aspace
            .find_free_area(len)
            .ok_or(crate::err!(ENOMEM))?
    };

    let prot = to_prot(prot);
    // PROT_NONE regions still need a VMA so a later `mprotect` can find them,
    // but must fault on any access; `Prot::empty()` gives exactly that.
    aspace.map_region(start, start + len, prot, backing, shared, name);

    if flags & MAP_POPULATE != 0 && !prot.is_empty() {
        aspace.populate(start, start + len, prot.contains(Prot::WRITE));
    }

    Ok(start as isize)
}

pub fn sys_munmap(addr: usize, len: usize) -> Result<isize> {
    if len == 0 || !mm::is_page_aligned(addr) {
        bail!(EINVAL);
    }
    let task = task::current();
    task.aspace.lock().unmap_range(addr, addr + mm::page_up(len));
    Ok(0)
}

pub fn sys_mprotect(addr: usize, len: usize, prot: u32) -> Result<isize> {
    if !mm::is_page_aligned(addr) {
        bail!(EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    let end = addr + mm::page_up(len);
    let task = task::current();
    let mut aspace = task.aspace.lock();
    // The whole range must be mapped.
    if aspace.find_vma(addr).is_none() {
        bail!(ENOMEM);
    }
    aspace.protect_range(addr, end, to_prot(prot));
    Ok(0)
}

const MREMAP_MAYMOVE: u32 = 1;
const MREMAP_FIXED: u32 = 2;

pub fn sys_mremap(
    old_addr: usize,
    old_len: usize,
    new_len: usize,
    flags: u32,
    new_addr: usize,
) -> Result<isize> {
    if !mm::is_page_aligned(old_addr) || new_len == 0 {
        bail!(EINVAL);
    }
    let old_len = mm::page_up(old_len);
    let new_len = mm::page_up(new_len);
    let task = task::current();
    let mut aspace = task.aspace.lock();

    let Some(vma) = aspace.find_vma(old_addr).cloned() else {
        bail!(EINVAL);
    };
    if old_addr + old_len > vma.end {
        bail!(EINVAL);
    }

    // Shrinking in place is always possible.
    if new_len <= old_len {
        if new_len < old_len {
            aspace.unmap_range(old_addr + new_len, old_addr + old_len);
        }
        return Ok(old_addr as isize);
    }

    // Try to grow in place, keeping the pages already populated.
    if flags & MREMAP_FIXED == 0
        && vma.start == old_addr
        && aspace.resize_vma(old_addr, old_addr + new_len)
    {
        return Ok(old_addr as isize);
    }

    if flags & MREMAP_MAYMOVE == 0 {
        bail!(ENOMEM);
    }

    // Move it: pick a new range, map it, copy the contents, drop the old one.
    let target = if flags & MREMAP_FIXED != 0 {
        if !mm::is_page_aligned(new_addr) {
            bail!(EINVAL);
        }
        new_addr
    } else {
        aspace.find_free_area(new_len).ok_or(crate::err!(ENOMEM))?
    };

    aspace.map_region(
        target,
        target + new_len,
        vma.prot,
        vma.backing.clone(),
        vma.shared,
        vma.name,
    );
    // Fault in both ranges and copy the old contents across.
    if !aspace.populate(target, target + old_len, true) {
        bail!(ENOMEM);
    }
    if !aspace.populate(old_addr, old_addr + old_len, false) {
        bail!(ENOMEM);
    }
    unsafe {
        core::ptr::copy(old_addr as *const u8, target as *mut u8, old_len);
    }
    aspace.unmap_range(old_addr, old_addr + old_len);
    Ok(target as isize)
}

const MADV_DONTNEED: u32 = 4;
const MADV_FREE: u32 = 8;

pub fn sys_madvise(addr: usize, len: usize, advice: u32) -> Result<isize> {
    // `MADV_DONTNEED` must actually discard the pages: musl's malloc relies on
    // getting zeros back when it re-touches them.
    if advice == MADV_DONTNEED || advice == MADV_FREE {
        if len == 0 {
            return Ok(0);
        }
        let task = task::current();
        let mut aspace = task.aspace.lock();
        let start = mm::page_down(addr);
        let end = mm::page_up(addr + len);
        // Keep the VMA, drop only the populated pages, so the next access
        // faults in a fresh zero page.
        let Some(vma) = aspace.find_vma(start).cloned() else {
            return Ok(0);
        };
        // Only anonymous private mappings can be discarded safely.
        if matches!(vma.backing, Backing::Anon) && !vma.shared {
            let mut va = start;
            while va < end.min(vma.end) {
                if let Some(pa) = aspace.page_table.unmap(va) {
                    crate::mm::frame::decref(pa);
                }
                va += mm::PAGE_SIZE;
            }
            crate::mm::page_table::flush_tlb_all();
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// System V shared memory
// ---------------------------------------------------------------------------

/// A shared memory segment: a set of frames that stay alive as long as the
/// segment exists.
struct ShmSegment {
    id: i32,
    key: usize,
    size: usize,
    frames: alloc::vec::Vec<usize>,
    /// Marked for deletion by `IPC_RMID`.
    removed: bool,
    attached: usize,
}

static SEGMENTS: Mutex<BTreeMap<i32, Arc<Mutex<ShmSegment>>>> = Mutex::new(BTreeMap::new());
static NEXT_SHM_ID: Mutex<i32> = Mutex::new(1);

const IPC_CREAT: u32 = 0o1000;
const IPC_EXCL: u32 = 0o2000;
const IPC_RMID: u32 = 0;
const IPC_STAT: u32 = 2;
const IPC_SET: u32 = 1;

pub fn sys_shmget(key: usize, size: usize, flags: u32) -> Result<isize> {
    let size = mm::page_up(size.max(1));
    let mut segments = SEGMENTS.lock();

    // An existing segment with this key?
    if key != 0 {
        if let Some((&id, seg)) = segments.iter().find(|(_, s)| s.lock().key == key) {
            if flags & IPC_EXCL != 0 {
                bail!(EEXIST);
            }
            if seg.lock().size < size {
                bail!(EINVAL);
            }
            return Ok(id as isize);
        }
    }
    if flags & IPC_CREAT == 0 && key != 0 {
        bail!(ENOENT);
    }

    // Allocate the frames up front so the segment is genuinely shared.
    let pages = size / mm::PAGE_SIZE;
    let mut frames = alloc::vec::Vec::with_capacity(pages);
    for _ in 0..pages {
        match crate::mm::frame::alloc_frame() {
            Some(pa) => frames.push(pa),
            None => {
                for pa in frames {
                    crate::mm::frame::decref(pa);
                }
                bail!(ENOMEM);
            }
        }
    }

    let mut next = NEXT_SHM_ID.lock();
    let id = *next;
    *next += 1;
    drop(next);

    segments.insert(
        id,
        Arc::new(Mutex::new(ShmSegment {
            id,
            key,
            size,
            frames,
            removed: false,
            attached: 0,
        })),
    );
    Ok(id as isize)
}

pub fn sys_shmat(id: i32, addr: usize, _flags: u32) -> Result<isize> {
    let segment = SEGMENTS
        .lock()
        .get(&id)
        .cloned()
        .ok_or(crate::err!(EINVAL))?;
    let mut seg = segment.lock();
    let task = task::current();
    let mut aspace = task.aspace.lock();

    let start = if addr != 0 {
        if !mm::is_page_aligned(addr) {
            bail!(EINVAL);
        }
        addr
    } else {
        aspace.find_free_area(seg.size).ok_or(crate::err!(ENOMEM))?
    };

    // Map the segment's frames directly, so all attachers share them.
    aspace.map_region(
        start,
        start + seg.size,
        Prot::READ | Prot::WRITE,
        Backing::Anon,
        true,
        "[shm]",
    );
    let flags = crate::mm::page_table::PTEFlags::V
        | crate::mm::page_table::PTEFlags::R
        | crate::mm::page_table::PTEFlags::W
        | crate::mm::page_table::PTEFlags::U
        | crate::mm::page_table::PTEFlags::A
        | crate::mm::page_table::PTEFlags::D;
    for (i, &pa) in seg.frames.iter().enumerate() {
        crate::mm::frame::incref(pa);
        aspace
            .page_table
            .map(start + i * mm::PAGE_SIZE, pa, flags)
            .ok_or(crate::err!(ENOMEM))?;
    }
    crate::mm::page_table::flush_tlb_all();
    seg.attached += 1;
    Ok(start as isize)
}

pub fn sys_shmdt(addr: usize) -> Result<isize> {
    if !mm::is_page_aligned(addr) {
        bail!(EINVAL);
    }
    let task = task::current();
    let mut aspace = task.aspace.lock();
    let Some(vma) = aspace.find_vma(addr).cloned() else {
        bail!(EINVAL);
    };
    aspace.unmap_range(vma.start, vma.end);
    Ok(0)
}

/// `struct shmid_ds`, truncated to the fields anything reads.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ShmidDs {
    // struct ipc_perm
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: u16,
    _pad1: u16,
    seq: u16,
    _pad2: u16,
    _pad3: u64,
    segsz: u64,
    atime: i64,
    dtime: i64,
    ctime: i64,
    cpid: i32,
    lpid: i32,
    nattch: u64,
    _unused: [u64; 2],
}

pub fn sys_shmctl(id: i32, cmd: u32, buf: usize) -> Result<isize> {
    match cmd {
        IPC_RMID => {
            let segment = SEGMENTS.lock().remove(&id).ok_or(crate::err!(EINVAL))?;
            let mut seg = segment.lock();
            seg.removed = true;
            // Release our references; attached mappings keep their own.
            for &pa in &seg.frames {
                crate::mm::frame::decref(pa);
            }
            seg.frames.clear();
            Ok(0)
        }
        IPC_STAT => {
            let segment = SEGMENTS
                .lock()
                .get(&id)
                .cloned()
                .ok_or(crate::err!(EINVAL))?;
            let seg = segment.lock();
            let mut ds = ShmidDs::default();
            ds.key = seg.key as i32;
            ds.segsz = seg.size as u64;
            ds.mode = 0o600;
            ds.nattch = seg.attached as u64;
            ds.cpid = task::current().pid() as i32;
            let _ = seg.id;
            crate::mm::uaccess::write(buf, ds)?;
            Ok(0)
        }
        IPC_SET => Ok(0),
        _ => bail!(EINVAL),
    }
}

/// Keep the constants documented even where unused by our code paths.
const _: u32 = MAP_PRIVATE | MAP_NORESERVE | MAP_GROWSDOWN | PROT_NONE;
