// Minimal ELF64 (RISC-V) loader for a single-address-space kernel.
pub const PT_LOAD: u32 = 1;
pub const PT_INTERP: u32 = 3;

#[derive(Clone, Copy)]
pub struct Loaded {
    pub base: usize,
    pub entry: usize,
    pub phdr: usize,
    pub phnum: usize,
    pub phent: usize,
    pub end: usize,
}

fn rd16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn rd32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn rd64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        d[o],
        d[o + 1],
        d[o + 2],
        d[o + 3],
        d[o + 4],
        d[o + 5],
        d[o + 6],
        d[o + 7],
    ])
}

pub fn interp_path(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return None;
    }
    let phoff = rd64(data, 32) as usize;
    let phentsize = rd16(data, 54) as usize;
    let phnum = rd16(data, 56) as usize;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        if o + 56 > data.len() {
            return None;
        }
        if rd32(data, o) == PT_INTERP {
            let off = rd64(data, o + 8) as usize;
            let sz = rd64(data, o + 32) as usize;
            if off + sz <= data.len() {
                let s = &data[off..off + sz];
                let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
                return Some(&s[..end]);
            }
        }
    }
    None
}

pub fn load(data: &[u8], base_hint: usize) -> Result<Loaded, &'static str> {
    if data.len() < 64 || &data[0..4] != b"\x7fELF" {
        return Err("not elf");
    }
    if data[4] != 2 {
        return Err("not elf64");
    }
    if rd16(data, 18) != 243 {
        return Err("not riscv");
    }
    let e_type = rd16(data, 16);
    let e_entry = rd64(data, 24) as usize;
    let phoff = rd64(data, 32) as usize;
    let phentsize = rd16(data, 54) as usize;
    let phnum = rd16(data, 56) as usize;
    if phnum == 0 || phnum > 64 {
        return Err("bad phnum");
    }
    let mut min_vaddr = usize::MAX;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        if o + 56 > data.len() {
            return Err("phdr oob");
        }
        if rd32(data, o) == PT_LOAD {
            let v = rd64(data, o + 16) as usize;
            if v < min_vaddr {
                min_vaddr = v;
            }
        }
    }
    if min_vaddr == usize::MAX {
        return Err("no load");
    }
    let bias = match e_type {
        3 => base_hint.wrapping_sub(min_vaddr & !0xfff),
        _ => {
            if min_vaddr < 0x10000 {
                return Err("non-pie low vaddr");
            }
            0
        }
    };
    let mut end = 0usize;
    for i in 0..phnum {
        let o = phoff + i * phentsize;
        if rd32(data, o) != PT_LOAD {
            continue;
        }
        let offset = rd64(data, o + 8) as usize;
        let vaddr = rd64(data, o + 16) as usize;
        let filesz = rd64(data, o + 32) as usize;
        let memsz = rd64(data, o + 40) as usize;
        if offset + filesz > data.len() || memsz < filesz {
            return Err("segment oob");
        }
        let dest = bias.wrapping_add(vaddr);
        if dest < 0x1000 || dest + memsz > 0x1_0000_0000 {
            return Err("segment addr out of range");
        }
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(offset), dest as *mut u8, filesz);
            core::ptr::write_bytes((dest + filesz) as *mut u8, 0, memsz - filesz);
        }
        if dest + memsz > end {
            end = dest + memsz;
        }
    }
    crate::arch::fence_i();
    Ok(Loaded {
        base: bias,
        entry: bias.wrapping_add(e_entry),
        phdr: bias.wrapping_add(phoff),
        phnum,
        phent: phentsize,
        end,
    })
}
