use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::memory::{self, PTE_R, PTE_W, PTE_X};

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_RISCV: u16 = 243;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const MAIN_BASE: usize = 0x10_0000_0000;
const INTERP_BASE: usize = 0x20_0000_0000;
const STACK_TOP: usize = 0x3f00_0000;
const STACK_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct ElfError(pub &'static str);

struct Header {
    kind: u16,
    entry: usize,
    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

struct ProgramHeader {
    kind: u32,
    flags: u32,
    offset: usize,
    virtual_address: usize,
    file_size: usize,
    memory_size: usize,
}

struct LoadedElf {
    base: usize,
    entry: usize,
    phdr: usize,
    phentsize: usize,
    phnum: usize,
    load_end: usize,
    interpreter: Option<String>,
}

pub struct ProcessImage {
    pub entry: usize,
    pub stack: usize,
    pub program_break: usize,
    pub mmap_next: usize,
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, ElfError> {
    let bytes = data.get(offset..offset + 2).ok_or(ElfError("short ELF u16"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, ElfError> {
    let bytes = data.get(offset..offset + 4).ok_or(ElfError("short ELF u32"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn u64_at(data: &[u8], offset: usize) -> Result<usize, ElfError> {
    let bytes = data.get(offset..offset + 8).ok_or(ElfError("short ELF u64"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()) as usize)
}

fn header(data: &[u8]) -> Result<Header, ElfError> {
    if data.len() < 64
        || data.get(0..4) != Some(b"\x7fELF")
        || data[4] != 2
        || data[5] != 1
    {
        return Err(ElfError("unsupported ELF identification"));
    }
    if u16_at(data, 18)? != EM_RISCV {
        return Err(ElfError("ELF is not RISC-V"));
    }
    let phentsize = u16_at(data, 54)? as usize;
    if phentsize != 56 {
        return Err(ElfError("unexpected program header size"));
    }
    Ok(Header {
        kind: u16_at(data, 16)?,
        entry: u64_at(data, 24)?,
        phoff: u64_at(data, 32)?,
        phentsize,
        phnum: u16_at(data, 56)? as usize,
    })
}

fn program_header(data: &[u8], header: &Header, index: usize) -> Result<ProgramHeader, ElfError> {
    let offset = header
        .phoff
        .checked_add(index * header.phentsize)
        .ok_or(ElfError("program header overflow"))?;
    Ok(ProgramHeader {
        kind: u32_at(data, offset)?,
        flags: u32_at(data, offset + 4)?,
        offset: u64_at(data, offset + 8)?,
        virtual_address: u64_at(data, offset + 16)?,
        file_size: u64_at(data, offset + 32)?,
        memory_size: u64_at(data, offset + 40)?,
    })
}

fn protection(flags: u32) -> usize {
    let mut result = 0;
    if flags & 4 != 0 {
        result |= PTE_R;
    }
    if flags & 2 != 0 {
        result |= PTE_W;
    }
    if flags & 1 != 0 {
        result |= PTE_X;
    }
    result
}

fn load_elf(data: &[u8], dynamic_base: usize) -> Result<LoadedElf, ElfError> {
    let header = header(data)?;
    let base = match header.kind {
        ET_EXEC => 0,
        ET_DYN => dynamic_base,
        _ => return Err(ElfError("ELF is neither executable nor PIE")),
    };
    let mut phdr = None;
    let mut interpreter = None;
    let mut load_end = 0usize;
    let mut segments = Vec::new();
    for index in 0..header.phnum {
        let segment = program_header(data, &header, index)?;
        match segment.kind {
            PT_PHDR => phdr = Some(base + segment.virtual_address),
            PT_INTERP => {
                let raw = data
                    .get(segment.offset..segment.offset + segment.file_size)
                    .ok_or(ElfError("short interpreter path"))?;
                let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
                let path = core::str::from_utf8(&raw[..end])
                    .map_err(|_| ElfError("interpreter path is not UTF-8"))?;
                interpreter = Some(String::from(path));
            }
            PT_LOAD => {
                if segment.file_size > segment.memory_size {
                    return Err(ElfError("ELF file size exceeds memory size"));
                }
                data.get(segment.offset..segment.offset + segment.file_size)
                    .ok_or(ElfError("short load segment"))?;
                load_end = load_end.max(base + segment.virtual_address + segment.memory_size);
                segments.push(segment);
            }
            _ => {}
        }
    }
    if segments.is_empty() {
        return Err(ElfError("ELF has no loadable segments"));
    }
    if phdr.is_none() {
        for segment in &segments {
            if header.phoff >= segment.offset
                && header.phoff + header.phnum * header.phentsize
                    <= segment.offset + segment.file_size
            {
                phdr = Some(base + segment.virtual_address + header.phoff - segment.offset);
            }
        }
    }
    for segment in segments {
        let address = base + segment.virtual_address;
        memory::map_user(address, segment.memory_size, protection(segment.flags))
            .map_err(|_| ElfError("cannot map ELF segment"))?;
        memory::loader_write(
            address,
            &data[segment.offset..segment.offset + segment.file_size],
        )
        .map_err(|_| ElfError("cannot copy ELF segment"))?;
    }
    Ok(LoadedElf {
        base,
        entry: base + header.entry,
        phdr: phdr.ok_or(ElfError("cannot locate program headers"))?,
        phentsize: header.phentsize,
        phnum: header.phnum,
        load_end,
        interpreter,
    })
}

fn push_bytes(stack: &mut usize, bytes: &[u8]) -> usize {
    *stack -= bytes.len();
    memory::copy_to_user(*stack, bytes).expect("write initial user stack");
    *stack
}

pub fn load(path: &str, arguments: &[&str], environment: &[&str]) -> Result<ProcessImage, ElfError> {
    let executable = crate::fs::read_file(path).map_err(|_| ElfError("cannot read executable"))?;
    let main = load_elf(&executable, MAIN_BASE)?;
    let interpreter = match &main.interpreter {
        Some(path) => {
            let data = crate::fs::read_file(path).map_err(|_| ElfError("cannot read musl loader"))?;
            let loaded = load_elf(&data, INTERP_BASE)?;
            crate::println!(
                "elf: interpreter={path} base={:#x} entry={:#x}",
                loaded.base,
                loaded.entry
            );
            Some(loaded)
        }
        None => None,
    };
    let entry = interpreter.as_ref().map_or(main.entry, |image| image.entry);
    let interpreter_base = interpreter.as_ref().map_or(0, |image| image.base);

    let stack_bottom = STACK_TOP - STACK_SIZE;
    memory::map_user(stack_bottom, STACK_SIZE, PTE_R | PTE_W)
        .map_err(|_| ElfError("cannot map user stack"))?;
    let mut string_pointer = STACK_TOP;
    let mut argv = Vec::new();
    for argument in arguments {
        let mut bytes = Vec::from(argument.as_bytes());
        bytes.push(0);
        let pointer = push_bytes(&mut string_pointer, &bytes);
        argv.push(pointer);
    }
    let mut envp = Vec::new();
    for variable in environment {
        let mut bytes = Vec::from(variable.as_bytes());
        bytes.push(0);
        envp.push(push_bytes(&mut string_pointer, &bytes));
    }
    let executable_name = push_bytes(&mut string_pointer, b"/usr/sbin/nginx\0");
    let mut random = [0u8; 16];
    let mut state = crate::arch::time() | 1;
    for byte in &mut random {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    let random_pointer = push_bytes(&mut string_pointer, &random);

    let mut words = vec![argv.len()];
    words.extend(argv.iter().copied());
    words.push(0);
    words.extend(envp.iter().copied());
    words.push(0);
    let auxiliary = [
        (3, main.phdr),
        (4, main.phentsize),
        (5, main.phnum),
        (6, memory::PAGE_SIZE),
        (7, interpreter_base),
        (8, 0),
        (9, main.entry),
        (11, 0),
        (12, 0),
        (13, 0),
        (14, 0),
        (16, 0),
        (17, 100),
        (23, 0),
        (25, random_pointer),
        (31, executable_name),
    ];
    for (key, value) in auxiliary {
        words.push(key);
        words.push(value);
    }
    words.extend([0, 0]);
    let stack = memory::align_down(
        string_pointer - words.len() * size_of::<usize>(),
        16,
    );
    for (index, word) in words.iter().enumerate() {
        memory::write_user(stack + index * size_of::<usize>(), word)
            .map_err(|_| ElfError("cannot write stack vectors"))?;
    }
    crate::println!(
        "elf: main_base={:#x} entry={:#x} initial_entry={:#x} stack={:#x} brk={:#x}",
        main.base,
        main.entry,
        entry,
        stack,
        main.load_end
    );
    Ok(ProcessImage {
        entry,
        stack,
        program_break: main.load_end,
        mmap_next: 0x30_0000_0000,
    })
}
