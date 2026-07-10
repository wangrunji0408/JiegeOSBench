use crate::memory;

const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rootfs.jgfs"));
const MAX_FDS: usize = 64;

#[derive(Clone, Copy)]
struct Entry {
    kind: u8,
    data_offset: usize,
    size: usize,
}

#[derive(Clone, Copy)]
struct Descriptor {
    used: bool,
    entry: Entry,
    position: usize,
}

const EMPTY_DESCRIPTOR: Descriptor = Descriptor {
    used: false,
    entry: Entry {
        kind: 0,
        data_offset: 0,
        size: 0,
    },
    position: 0,
};

static mut DESCRIPTORS: [Descriptor; MAX_FDS] = [EMPTY_DESCRIPTOR; MAX_FDS];

fn read_u16(offset: usize) -> u16 {
    u16::from_le_bytes(ARCHIVE[offset..offset + 2].try_into().unwrap())
}
fn read_u64(offset: usize) -> u64 {
    u64::from_le_bytes(ARCHIVE[offset..offset + 8].try_into().unwrap())
}

fn lookup_once(path: &[u8]) -> Option<Entry> {
    if &ARCHIVE[..8] != b"JIEGEFS1" {
        return None;
    }
    let mut cursor = 8;
    loop {
        let name_length = read_u16(cursor) as usize;
        cursor += 2;
        if name_length == 0 {
            return None;
        }
        let kind = ARCHIVE[cursor];
        let size = read_u64(cursor + 2) as usize;
        cursor += 10;
        let name = &ARCHIVE[cursor..cursor + name_length];
        cursor += name_length;
        let data_offset = cursor;
        cursor += size;
        if name == path {
            return Some(Entry {
                kind,
                data_offset,
                size,
            });
        }
    }
}

fn lookup(path: &[u8]) -> Option<Entry> {
    let mut current = [0u8; 512];
    if path.len() > current.len() {
        return None;
    }
    current[..path.len()].copy_from_slice(path);
    let mut current_length = path.len();
    for _ in 0..8 {
        let entry = lookup_once(&current[..current_length])?;
        if entry.kind != 2 {
            return Some(entry);
        }
        let target = &ARCHIVE[entry.data_offset..entry.data_offset + entry.size];
        if target.starts_with(b"/") {
            if target.len() > current.len() {
                return None;
            }
            current[..target.len()].copy_from_slice(target);
            current_length = target.len();
        } else {
            let slash = current[..current_length]
                .iter()
                .rposition(|byte| *byte == b'/')?
                + 1;
            if slash + target.len() > current.len() {
                return None;
            }
            current[slash..slash + target.len()].copy_from_slice(target);
            current_length = slash + target.len();
        }
    }
    None
}

pub fn file(path: &[u8]) -> Option<&'static [u8]> {
    let entry = lookup(path)?;
    (entry.kind == 1).then(|| &ARCHIVE[entry.data_offset..entry.data_offset + entry.size])
}

pub fn path_metadata(path: &[u8]) -> Option<(usize, usize)> {
    let entry = lookup(path)?;
    Some((entry.size, entry.data_offset))
}

pub fn init() {
    let mut files = 0;
    let mut bytes = 0;
    let mut cursor = 8;
    loop {
        let name_length = read_u16(cursor) as usize;
        cursor += 2;
        if name_length == 0 {
            break;
        }
        let size = read_u64(cursor + 2) as usize;
        cursor += 10 + name_length + size;
        files += 1;
        bytes += size;
    }
    crate::println!("initramfs: {} files, {} KiB", files, bytes / 1024);
}

pub fn open(path: &[u8]) -> isize {
    let Some(entry) = lookup(path) else {
        return -2;
    };
    let descriptors = &raw mut DESCRIPTORS;
    for fd in 3..MAX_FDS {
        unsafe {
            if !(*descriptors)[fd].used {
                (*descriptors)[fd] = Descriptor {
                    used: true,
                    entry,
                    position: 0,
                };
                return fd as isize;
            }
        }
    }
    -24
}

pub fn create_sink() -> isize {
    let descriptors = &raw mut DESCRIPTORS;
    for fd in 3..MAX_FDS {
        unsafe {
            if !(*descriptors)[fd].used {
                (*descriptors)[fd] = Descriptor {
                    used: true,
                    entry: Entry {
                        kind: 3,
                        data_offset: 0,
                        size: 0,
                    },
                    position: 0,
                };
                return fd as isize;
            }
        }
    }
    -24
}

pub fn write_sink(fd: usize, length: usize) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw mut DESCRIPTORS;
    unsafe {
        let descriptor = &mut (*descriptors)[fd];
        if !descriptor.used || descriptor.entry.kind != 3 {
            return -9;
        }
        descriptor.position += length;
        length as isize
    }
}

pub fn close(fd: usize) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw mut DESCRIPTORS;
    unsafe {
        if !(*descriptors)[fd].used {
            return -9;
        }
        (*descriptors)[fd] = EMPTY_DESCRIPTOR;
    }
    0
}

pub fn metadata(fd: usize) -> Option<(usize, usize)> {
    if fd < 3 || fd >= MAX_FDS {
        return None;
    }
    let descriptors = &raw const DESCRIPTORS;
    unsafe {
        let descriptor = (*descriptors)[fd];
        descriptor
            .used
            .then_some((descriptor.entry.size, descriptor.entry.data_offset))
    }
}

pub fn read(fd: usize, user_buffer: usize, length: usize) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw mut DESCRIPTORS;
    unsafe {
        let descriptor = &mut (*descriptors)[fd];
        if !descriptor.used {
            return -9;
        }
        let available = descriptor.entry.size.saturating_sub(descriptor.position);
        let count = length.min(available);
        for index in 0..count {
            let byte = ARCHIVE[descriptor.entry.data_offset + descriptor.position + index];
            if !memory::write_user_byte(user_buffer + index, byte) {
                return -14;
            }
        }
        descriptor.position += count;
        count as isize
    }
}

pub fn read_kernel(fd: usize, output: &mut [u8]) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw mut DESCRIPTORS;
    unsafe {
        let descriptor = &mut (*descriptors)[fd];
        if !descriptor.used || descriptor.entry.kind != 1 {
            return -9;
        }
        let count = output
            .len()
            .min(descriptor.entry.size.saturating_sub(descriptor.position));
        output[..count].copy_from_slice(
            &ARCHIVE[descriptor.entry.data_offset + descriptor.position
                ..descriptor.entry.data_offset + descriptor.position + count],
        );
        descriptor.position += count;
        count as isize
    }
}

pub fn pread(fd: usize, user_buffer: usize, length: usize, offset: usize) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw const DESCRIPTORS;
    unsafe {
        let descriptor = (*descriptors)[fd];
        if !descriptor.used {
            return -9;
        }
        let count = length.min(descriptor.entry.size.saturating_sub(offset));
        for index in 0..count {
            let byte = ARCHIVE[descriptor.entry.data_offset + offset + index];
            if !memory::write_user_byte(user_buffer + index, byte) {
                return -14;
            }
        }
        count as isize
    }
}

pub fn seek(fd: usize, offset: isize, whence: usize) -> isize {
    if fd < 3 || fd >= MAX_FDS {
        return -9;
    }
    let descriptors = &raw mut DESCRIPTORS;
    unsafe {
        let descriptor = &mut (*descriptors)[fd];
        if !descriptor.used {
            return -9;
        }
        let base = match whence {
            0 => 0,
            1 => descriptor.position,
            2 => descriptor.entry.size,
            _ => return -22,
        };
        let Some(position) = base.checked_add_signed(offset) else {
            return -22;
        };
        descriptor.position = position;
        position as isize
    }
}

pub fn pread_to_physical(fd: usize, offset: usize, destination: usize, length: usize) -> usize {
    if fd < 3 || fd >= MAX_FDS {
        return 0;
    }
    let descriptors = &raw const DESCRIPTORS;
    unsafe {
        let descriptor = (*descriptors)[fd];
        if !descriptor.used {
            return 0;
        }
        let count = length.min(descriptor.entry.size.saturating_sub(offset));
        core::ptr::copy_nonoverlapping(
            ARCHIVE.as_ptr().add(descriptor.entry.data_offset + offset),
            destination as *mut u8,
            count,
        );
        count
    }
}

pub fn read_path_from_user(address: usize, output: &mut [u8]) -> Result<usize, isize> {
    for index in 0..output.len() {
        let Some(byte) = memory::read_user_byte(address + index) else {
            return Err(-14);
        };
        if byte == 0 {
            return Ok(index);
        }
        output[index] = byte;
    }
    Err(-36)
}
