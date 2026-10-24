use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::{self, File, Metadata};

const EBADF: i32 = 9;
const EINVAL: i32 = 22;
const ESPIPE: i32 = 29;

enum Descriptor {
    Stdin,
    Stdout,
    Stderr,
    File(File),
}

static DESCRIPTORS: Mutex<Option<BTreeMap<i32, Descriptor>>> = Mutex::new(None);

pub fn init() {
    let mut descriptors = BTreeMap::new();
    descriptors.insert(0, Descriptor::Stdin);
    descriptors.insert(1, Descriptor::Stdout);
    descriptors.insert(2, Descriptor::Stderr);
    *DESCRIPTORS.lock() = Some(descriptors);
}

fn allocate(descriptors: &BTreeMap<i32, Descriptor>) -> i32 {
    let mut candidate = 0;
    while descriptors.contains_key(&candidate) {
        candidate += 1;
    }
    candidate
}

pub fn insert_file(file: File) -> i32 {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    let fd = allocate(descriptors);
    descriptors.insert(fd, Descriptor::File(file));
    fd
}

pub fn open(path: &str, flags: u32, mode: u32) -> Result<i32, i32> {
    Ok(insert_file(fs::open(path, flags, mode)?))
}

pub fn close(fd: i32) -> Result<(), i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    descriptors.remove(&fd).map(|_| ()).ok_or(EBADF)
}

pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::Stdin => Ok(0),
        Descriptor::Stdout | Descriptor::Stderr => Err(EBADF),
        Descriptor::File(file) => file.read(buffer),
    }
}

pub fn write(fd: i32, buffer: &[u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::Stdin => Err(EBADF),
        Descriptor::Stdout | Descriptor::Stderr => {
            crate::console::_print(format_args!("{}", String::from_utf8_lossy(buffer)));
            Ok(buffer.len())
        }
        Descriptor::File(file) => file.write(buffer),
    }
}

pub fn read_at(fd: i32, offset: u64, buffer: &mut [u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => {
            let previous = file.seek(0, 1)?;
            if offset > i64::MAX as u64 {
                return Err(EINVAL);
            }
            file.seek(offset as i64, 0)?;
            let result = file.read(buffer);
            file.seek(previous as i64, 0)?;
            result
        }
        _ => Err(ESPIPE),
    }
}

pub fn write_at(fd: i32, offset: u64, buffer: &[u8]) -> Result<usize, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => {
            let previous = file.seek(0, 1)?;
            if offset > i64::MAX as u64 {
                return Err(EINVAL);
            }
            file.seek(offset as i64, 0)?;
            let result = file.write(buffer);
            file.seek(previous as i64, 0)?;
            result
        }
        _ => Err(ESPIPE),
    }
}

pub fn seek(fd: i32, offset: i64, whence: u32) -> Result<u64, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    match descriptors.get_mut(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => file.seek(offset, whence),
        _ => Err(ESPIPE),
    }
}

pub fn metadata(fd: i32) -> Result<Metadata, i32> {
    let guard = DESCRIPTORS.lock();
    let descriptors = guard.as_ref().unwrap();
    match descriptors.get(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => file.metadata(),
        _ => Ok(Metadata {
            mode: 0o020600,
            size: 0,
            inode: 1,
        }),
    }
}

pub fn path(fd: i32) -> Result<String, i32> {
    let guard = DESCRIPTORS.lock();
    let descriptors = guard.as_ref().unwrap();
    match descriptors.get(&fd).ok_or(EBADF)? {
        Descriptor::File(file) => Ok(file.path().to_string()),
        _ => Err(EBADF),
    }
}

pub fn directory_entries(fd: i32) -> Result<Vec<(String, Metadata)>, i32> {
    let path = path(fd)?;
    fs::read_dir(&path)
}

pub fn directory_offset(fd: i32) -> Result<u64, i32> {
    seek(fd, 0, 1)
}

pub fn set_directory_offset(fd: i32, offset: u64) -> Result<(), i32> {
    if offset > i64::MAX as u64 {
        return Err(EINVAL);
    }
    seek(fd, offset as i64, 0).map(|_| ())
}

pub fn duplicate(fd: i32, requested: Option<i32>) -> Result<i32, i32> {
    let mut guard = DESCRIPTORS.lock();
    let descriptors = guard.as_mut().unwrap();
    let source = descriptors.get(&fd).ok_or(EBADF)?;
    let replacement = match source {
        Descriptor::Stdin => Descriptor::Stdin,
        Descriptor::Stdout => Descriptor::Stdout,
        Descriptor::Stderr => Descriptor::Stderr,
        Descriptor::File(_) => return Err(EINVAL),
    };
    let target = match requested {
        Some(target) => {
            if target < 0 {
                return Err(EBADF);
            }
            descriptors.remove(&target);
            target
        }
        None => allocate(descriptors),
    };
    descriptors.insert(target, replacement);
    Ok(target)
}
