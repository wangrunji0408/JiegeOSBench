//! Access to user memory of the current task. All accesses go through the page
//! table (faulting pages in on demand) and then touch physical memory via the
//! kernel's identity mapping, so bad user pointers surface as EFAULT rather
//! than kernel faults.
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;

use super::addrspace::{AccessKind, AddressSpace};
use crate::abi::{EFAULT, ENAMETOOLONG};
use crate::config::PAGE_SIZE;
use crate::sync::SpinLock;
use crate::task::current;

pub type Mm = Arc<SpinLock<AddressSpace>>;

pub fn current_mm() -> Mm {
    current().mm()
}

/// Visit user memory [addr, addr+len) as a sequence of kernel slices (page chunks).
pub fn for_each_chunk<F>(mm: &Mm, addr: usize, len: usize, write: bool, mut f: F) -> Result<(), i32>
where
    F: FnMut(&mut [u8]) -> Result<(), i32>,
{
    if len == 0 {
        return Ok(());
    }
    if addr.checked_add(len).is_none() {
        return Err(EFAULT);
    }
    let kind = if write { AccessKind::Write } else { AccessKind::Read };
    let mut cur = addr;
    let end = addr + len;
    while cur < end {
        let page_end = (cur & !(PAGE_SIZE - 1)) + PAGE_SIZE;
        let n = page_end.min(end) - cur;
        let pa = mm.lock().access(cur, kind).ok_or(EFAULT)?;
        let slice = unsafe { core::slice::from_raw_parts_mut(pa as *mut u8, n) };
        f(slice)?;
        cur += n;
    }
    Ok(())
}

pub fn copy_from_user_mm(mm: &Mm, dst: &mut [u8], src: usize) -> Result<(), i32> {
    let mut off = 0;
    for_each_chunk(mm, src, dst.len(), false, |chunk| {
        dst[off..off + chunk.len()].copy_from_slice(chunk);
        off += chunk.len();
        Ok(())
    })
}

pub fn copy_to_user_mm(mm: &Mm, dst: usize, src: &[u8]) -> Result<(), i32> {
    let mut off = 0;
    for_each_chunk(mm, dst, src.len(), true, |chunk| {
        chunk.copy_from_slice(&src[off..off + chunk.len()]);
        off += chunk.len();
        Ok(())
    })
}

pub fn copy_from_user(dst: &mut [u8], src: usize) -> Result<(), i32> {
    copy_from_user_mm(&current_mm(), dst, src)
}

pub fn copy_to_user(dst: usize, src: &[u8]) -> Result<(), i32> {
    copy_to_user_mm(&current_mm(), dst, src)
}

pub fn read_bytes(src: usize, len: usize) -> Result<Vec<u8>, i32> {
    let mut v = alloc::vec![0u8; len];
    copy_from_user(&mut v, src)?;
    Ok(v)
}

/// Read a plain-old-data value from user memory.
pub fn read_val<T: Copy>(addr: usize) -> Result<T, i32> {
    let mut v = core::mem::MaybeUninit::<T>::uninit();
    let buf = unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, size_of::<T>()) };
    copy_from_user(buf, addr)?;
    Ok(unsafe { v.assume_init() })
}

pub fn write_val<T: Copy>(addr: usize, val: T) -> Result<(), i32> {
    let buf = unsafe { core::slice::from_raw_parts(&val as *const T as *const u8, size_of::<T>()) };
    copy_to_user(addr, buf)
}

pub fn read_val_mm<T: Copy>(mm: &Mm, addr: usize) -> Result<T, i32> {
    let mut v = core::mem::MaybeUninit::<T>::uninit();
    let buf = unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, size_of::<T>()) };
    copy_from_user_mm(mm, buf, addr)?;
    Ok(unsafe { v.assume_init() })
}

pub fn write_val_mm<T: Copy>(mm: &Mm, addr: usize, val: T) -> Result<(), i32> {
    let buf = unsafe { core::slice::from_raw_parts(&val as *const T as *const u8, size_of::<T>()) };
    copy_to_user_mm(mm, addr, buf)
}

/// Read a NUL-terminated string (at most `max` bytes, excluding NUL).
pub fn read_cstr(addr: usize, max: usize) -> Result<Vec<u8>, i32> {
    let mm = current_mm();
    let mut out = Vec::new();
    let mut cur = addr;
    loop {
        let page_end = (cur & !(PAGE_SIZE - 1)) + PAGE_SIZE;
        let n = page_end - cur;
        let pa = mm.lock().access(cur, AccessKind::Read).ok_or(EFAULT)?;
        let slice = unsafe { core::slice::from_raw_parts(pa as *const u8, n) };
        if let Some(pos) = slice.iter().position(|&b| b == 0) {
            out.extend_from_slice(&slice[..pos]);
            if out.len() > max {
                return Err(ENAMETOOLONG);
            }
            return Ok(out);
        }
        out.extend_from_slice(slice);
        if out.len() > max {
            return Err(ENAMETOOLONG);
        }
        cur = page_end;
    }
}

pub fn read_string(addr: usize, max: usize) -> Result<String, i32> {
    let b = read_cstr(addr, max)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

/// Read a NULL-terminated array of user pointers to C strings (argv/envp).
pub fn read_str_array(addr: usize, max_items: usize) -> Result<Vec<Vec<u8>>, i32> {
    let mut out = Vec::new();
    if addr == 0 {
        return Ok(out);
    }
    for i in 0..max_items {
        let p: usize = read_val(addr + i * 8)?;
        if p == 0 {
            break;
        }
        out.push(read_cstr(p, 128 * 1024)?);
    }
    Ok(out)
}
