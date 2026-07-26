//! Access to user memory from the kernel.
//!
//! User pages are mapped in the same page table the kernel runs on, and
//! `sstatus.SUM` is set, so the kernel can dereference user pointers directly.
//! The only hazard is that a page may not be populated yet (lazy allocation),
//! so every accessor first faults the range in through the current task's
//! address space.

use super::addr::*;
use crate::task;
use alloc::string::String;
use alloc::vec::Vec;

/// Errors from user memory access, reported to user space as `EFAULT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault;

pub type Result<T> = core::result::Result<T, Fault>;

/// Make `[ptr, ptr + len)` present, checking it lies in user space.
fn ensure(ptr: usize, len: usize, write: bool) -> Result<()> {
    if len == 0 {
        return Ok(());
    }
    let end = ptr.checked_add(len).ok_or(Fault)?;
    if !is_user_addr(ptr) || end > USER_TOP {
        return Err(Fault);
    }
    let task = task::current();
    let mut aspace = task.aspace.lock();
    if aspace.populate(ptr, end, write) {
        Ok(())
    } else {
        Err(Fault)
    }
}

/// Copy `len` bytes out of user space.
pub fn read_bytes(ptr: usize, len: usize) -> Result<Vec<u8>> {
    ensure(ptr, len, false)?;
    let mut v = Vec::with_capacity(len);
    unsafe {
        v.set_len(len);
        core::ptr::copy_nonoverlapping(ptr as *const u8, v.as_mut_ptr(), len);
    }
    Ok(v)
}

/// Copy into a caller-provided buffer.
pub fn read_into(ptr: usize, buf: &mut [u8]) -> Result<()> {
    ensure(ptr, buf.len(), false)?;
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, buf.as_mut_ptr(), buf.len());
    }
    Ok(())
}

/// Copy `data` into user space.
pub fn write_bytes(ptr: usize, data: &[u8]) -> Result<()> {
    ensure(ptr, data.len(), true)?;
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    }
    Ok(())
}

/// Read a `T` from user space.
pub fn read<T: Copy>(ptr: usize) -> Result<T> {
    ensure(ptr, core::mem::size_of::<T>(), false)?;
    Ok(unsafe { (ptr as *const T).read_unaligned() })
}

/// Write a `T` to user space.
pub fn write<T: Copy>(ptr: usize, value: T) -> Result<()> {
    ensure(ptr, core::mem::size_of::<T>(), true)?;
    unsafe { (ptr as *mut T).write_unaligned(value) };
    Ok(())
}

/// Borrow a user buffer as a mutable slice (after faulting it in).
///
/// # Safety
/// The caller must not let the returned slice outlive the syscall, and must not
/// trigger a context switch that could unmap it while it is live.
pub unsafe fn slice_mut(ptr: usize, len: usize) -> Result<&'static mut [u8]> {
    ensure(ptr, len, true)?;
    Ok(core::slice::from_raw_parts_mut(ptr as *mut u8, len))
}

/// Borrow a user buffer as a shared slice.
///
/// # Safety
/// Same constraints as [`slice_mut`].
pub unsafe fn slice(ptr: usize, len: usize) -> Result<&'static [u8]> {
    ensure(ptr, len, false)?;
    Ok(core::slice::from_raw_parts(ptr as *const u8, len))
}

/// The longest C string we will read from user space.
const MAX_STR: usize = 64 * 1024;

/// Read a NUL-terminated string from user space.
pub fn read_cstr(ptr: usize) -> Result<String> {
    let bytes = read_cstr_bytes(ptr)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Read a NUL-terminated byte string from user space.
pub fn read_cstr_bytes(ptr: usize) -> Result<Vec<u8>> {
    if !is_user_addr(ptr) {
        return Err(Fault);
    }
    let mut out = Vec::new();
    let mut addr = ptr;
    // Fault in and scan one page at a time so we never read past a mapping.
    loop {
        let page_end = page_down(addr) + PAGE_SIZE;
        ensure(page_down(addr), PAGE_SIZE, false)?;
        while addr < page_end {
            let b = unsafe { *(addr as *const u8) };
            if b == 0 {
                return Ok(out);
            }
            out.push(b);
            addr += 1;
            if out.len() > MAX_STR {
                return Err(Fault);
            }
        }
        if addr >= USER_TOP {
            return Err(Fault);
        }
    }
}

/// Read a NULL-terminated array of string pointers (`argv` / `envp`).
pub fn read_cstr_array(mut ptr: usize) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    if ptr == 0 {
        return Ok(out);
    }
    loop {
        let p: usize = read(ptr)?;
        if p == 0 {
            return Ok(out);
        }
        out.push(read_cstr_bytes(p)?);
        ptr += core::mem::size_of::<usize>();
        if out.len() > 4096 {
            return Err(Fault);
        }
    }
}

/// A user `iovec`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoVec {
    pub base: usize,
    pub len: usize,
}

/// Read an array of iovecs from user space.
pub fn read_iovecs(ptr: usize, count: usize) -> Result<Vec<IoVec>> {
    if count > 1024 {
        return Err(Fault);
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        out.push(read::<IoVec>(ptr + i * core::mem::size_of::<IoVec>())?);
    }
    Ok(out)
}
