use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const PAGE_SIZE: usize = 4096;
pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

const RAM_START: usize = 0x8000_0000;
const RAM_END: usize = 0xa000_0000;
const PTE_PERM: usize = PTE_R | PTE_W | PTE_X | PTE_U;

static NEXT_FRAME: AtomicUsize = AtomicUsize::new(0);
static ROOT_PAGE_TABLE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static __kernel_end: u8;
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryError;

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Clone, Copy)]
pub struct Translation {
    pub physical: usize,
    pub flags: usize,
}

pub const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

pub const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

pub fn init() {
    crate::heap::init();
    let kernel_end = align_up(&raw const __kernel_end as usize, PAGE_SIZE);
    assert!((RAM_START..RAM_END).contains(&kernel_end));
    NEXT_FRAME.store(kernel_end, Ordering::Release);

    let root = alloc_frame().expect("initial page table");
    ROOT_PAGE_TABLE.store(root, Ordering::Release);
    let gigapage_flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    page_table(root)[2] = pte(RAM_START, gigapage_flags);

    map_supervisor_range(0x0200_0000, 0x1_0000);
    map_supervisor_range(0x0c00_0000, 0x400_0000);
    map_supervisor_range(0x1000_0000, 0x1000);
    map_supervisor_range(0x1000_1000, 0x8000);
    crate::println!(
        "memory: kernel_end={kernel_end:#x}, page_table={root:#x}, frames={}",
        (RAM_END - kernel_end) / PAGE_SIZE
    );
}

fn map_supervisor_range(physical: usize, length: usize) {
    for offset in (0..length).step_by(PAGE_SIZE) {
        map_page(
            physical + offset,
            physical + offset,
            PTE_R | PTE_W | PTE_G | PTE_A | PTE_D,
        )
        .expect("map supervisor MMIO");
    }
}

pub fn root() -> usize {
    ROOT_PAGE_TABLE.load(Ordering::Acquire)
}

pub fn activate() {
    let root = root();
    assert_ne!(root, 0);
    unsafe {
        asm!("sfence.vma");
        asm!("csrw satp, {}", in(reg) (8usize << 60) | (root >> 12));
        asm!("sfence.vma");
    }
}

pub fn alloc_frame() -> MemoryResult<usize> {
    let frame = NEXT_FRAME.fetch_add(PAGE_SIZE, Ordering::AcqRel);
    if frame + PAGE_SIZE > RAM_END {
        return Err(MemoryError);
    }
    unsafe { core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE) };
    Ok(frame)
}

fn page_table(physical: usize) -> &'static mut [usize; 512] {
    unsafe { &mut *(physical as *mut [usize; 512]) }
}

const fn pte(physical: usize, flags: usize) -> usize {
    ((physical >> 12) << 10) | flags
}

const fn pte_physical(entry: usize) -> usize {
    ((entry >> 10) << 12) & !(PAGE_SIZE - 1)
}

fn leaf_entry(root: usize, virtual_address: usize, create: bool) -> MemoryResult<*mut usize> {
    let mut table = root;
    for level in [2usize, 1] {
        let index = (virtual_address >> (12 + level * 9)) & 0x1ff;
        let entry = unsafe { &mut *(table as *mut usize).add(index) };
        if *entry & PTE_V == 0 {
            if !create {
                return Err(MemoryError);
            }
            let child = alloc_frame()?;
            *entry = pte(child, PTE_V);
        } else if *entry & (PTE_R | PTE_W | PTE_X) != 0 {
            return Err(MemoryError);
        }
        table = pte_physical(*entry);
    }
    let index = (virtual_address >> 12) & 0x1ff;
    Ok(unsafe { (table as *mut usize).add(index) })
}

pub fn map_page(virtual_address: usize, physical: usize, flags: usize) -> MemoryResult<()> {
    assert_eq!(virtual_address % PAGE_SIZE, 0);
    assert_eq!(physical % PAGE_SIZE, 0);
    let entry = leaf_entry(root(), virtual_address, true)?;
    unsafe {
        if *entry & PTE_V != 0 {
            return Err(MemoryError);
        }
        *entry = pte(physical, flags | PTE_V);
    }
    unsafe { asm!("sfence.vma {}", in(reg) virtual_address) };
    Ok(())
}

pub fn map_user(virtual_address: usize, length: usize, flags: usize) -> MemoryResult<()> {
    let start = align_down(virtual_address, PAGE_SIZE);
    let end = align_up(
        virtual_address.checked_add(length).ok_or(MemoryError)?,
        PAGE_SIZE,
    );
    for address in (start..end).step_by(PAGE_SIZE) {
        let entry = leaf_entry(root(), address, true)?;
        unsafe {
            if *entry & PTE_V != 0 {
                if *entry & PTE_U == 0 {
                    return Err(MemoryError);
                }
                *entry |= flags | PTE_A | PTE_D;
            } else {
                let physical = alloc_frame()?;
                *entry = pte(physical, flags | PTE_U | PTE_A | PTE_D);
            }
        }
    }
    unsafe { asm!("sfence.vma") };
    Ok(())
}

pub fn unmap_user(virtual_address: usize, length: usize) -> MemoryResult<()> {
    let start = align_down(virtual_address, PAGE_SIZE);
    let end = align_up(virtual_address + length, PAGE_SIZE);
    for address in (start..end).step_by(PAGE_SIZE) {
        let entry = leaf_entry(root(), address, false)?;
        unsafe {
            if *entry & PTE_U == 0 {
                return Err(MemoryError);
            }
            *entry = 0;
        }
    }
    unsafe { asm!("sfence.vma") };
    Ok(())
}

pub fn protect_user(virtual_address: usize, length: usize, flags: usize) -> MemoryResult<()> {
    let start = align_down(virtual_address, PAGE_SIZE);
    let end = align_up(virtual_address + length, PAGE_SIZE);
    for address in (start..end).step_by(PAGE_SIZE) {
        let entry = leaf_entry(root(), address, false)?;
        unsafe {
            if *entry & (PTE_V | PTE_U) != (PTE_V | PTE_U) {
                return Err(MemoryError);
            }
            *entry = (*entry & !PTE_PERM) | (flags & PTE_PERM);
        }
    }
    unsafe { asm!("sfence.vma") };
    Ok(())
}

pub fn translate(virtual_address: usize) -> MemoryResult<Translation> {
    let mut table = root();
    for level in [2usize, 1, 0] {
        let index = (virtual_address >> (12 + level * 9)) & 0x1ff;
        let entry = unsafe { core::ptr::read((table as *const usize).add(index)) };
        if entry & PTE_V == 0 {
            return Err(MemoryError);
        }
        if entry & (PTE_R | PTE_W | PTE_X) != 0 {
            let page_bits = 12 + level * 9;
            let offset = virtual_address & ((1usize << page_bits) - 1);
            return Ok(Translation {
                physical: pte_physical(entry) + offset,
                flags: entry,
            });
        }
        table = pte_physical(entry);
    }
    Err(MemoryError)
}

pub fn copy_to_user(virtual_address: usize, source: &[u8]) -> MemoryResult<()> {
    copy_user(virtual_address, source, true, false)
}

pub fn loader_write(virtual_address: usize, source: &[u8]) -> MemoryResult<()> {
    copy_user(virtual_address, source, true, false)
}

fn copy_user(
    virtual_address: usize,
    source: &[u8],
    write: bool,
    require_writable: bool,
) -> MemoryResult<()> {
    let mut copied = 0;
    while copied < source.len() {
        let address = virtual_address.checked_add(copied).ok_or(MemoryError)?;
        let translation = translate(address)?;
        if translation.flags & PTE_U == 0
            || (require_writable && translation.flags & PTE_W == 0)
        {
            return Err(MemoryError);
        }
        let count = (PAGE_SIZE - (address & (PAGE_SIZE - 1))).min(source.len() - copied);
        unsafe {
            if write {
                core::ptr::copy_nonoverlapping(
                    source.as_ptr().add(copied),
                    translation.physical as *mut u8,
                    count,
                );
            } else {
                core::ptr::copy_nonoverlapping(
                    translation.physical as *const u8,
                    source.as_ptr().add(copied) as *mut u8,
                    count,
                );
            }
        }
        copied += count;
    }
    Ok(())
}

pub fn copy_from_user(virtual_address: usize, destination: &mut [u8]) -> MemoryResult<()> {
    let mut copied = 0;
    while copied < destination.len() {
        let address = virtual_address.checked_add(copied).ok_or(MemoryError)?;
        let translation = translate(address)?;
        if translation.flags & PTE_U == 0 || translation.flags & PTE_R == 0 {
            return Err(MemoryError);
        }
        let count = (PAGE_SIZE - (address & (PAGE_SIZE - 1))).min(destination.len() - copied);
        unsafe {
            core::ptr::copy_nonoverlapping(
                translation.physical as *const u8,
                destination.as_mut_ptr().add(copied),
                count,
            );
        }
        copied += count;
    }
    Ok(())
}

pub fn read_user<T: Copy>(address: usize) -> MemoryResult<T> {
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    unsafe {
        copy_from_user(
            address,
            core::slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), size_of::<T>()),
        )?;
        Ok(value.assume_init())
    }
}

pub fn write_user<T: Copy>(address: usize, value: &T) -> MemoryResult<()> {
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>())
    };
    let mut copied = 0;
    while copied < bytes.len() {
        let current = address.checked_add(copied).ok_or(MemoryError)?;
        let translation = translate(current)?;
        if translation.flags & (PTE_U | PTE_W) != (PTE_U | PTE_W) {
            return Err(MemoryError);
        }
        let count = (PAGE_SIZE - (current & (PAGE_SIZE - 1))).min(bytes.len() - copied);
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(copied),
                translation.physical as *mut u8,
                count,
            );
        }
        copied += count;
    }
    Ok(())
}

pub fn read_user_string(address: usize, maximum: usize) -> MemoryResult<Vec<u8>> {
    let mut result = Vec::new();
    for offset in 0..maximum {
        let byte = read_user::<u8>(address + offset)?;
        if byte == 0 {
            return Ok(result);
        }
        result.push(byte);
    }
    Err(MemoryError)
}
