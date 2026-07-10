use crate::memory::{PAGE_SIZE, PTE_R, PTE_U, PTE_W, PTE_X, PageTable, alloc_frame};

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}
fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}
fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

pub struct LoadedElf {
    pub entry: usize,
    pub phdr: usize,
    pub phent: usize,
    pub phnum: usize,
}

pub fn load_at(image: &[u8], table: PageTable, base: usize) -> LoadedElf {
    assert!(
        image.len() >= 64 && &image[..4] == b"\x7fELF",
        "invalid ELF image"
    );
    assert_eq!(image[4], 2, "ELF is not 64-bit");
    assert_eq!(image[5], 1, "ELF is not little-endian");
    assert_eq!(u16_at(image, 18), 243, "ELF is not RISC-V");

    let entry = base + u64_at(image, 24) as usize;
    let phoff = u64_at(image, 32) as usize;
    let phentsize = u16_at(image, 54) as usize;
    let phnum = u16_at(image, 56) as usize;

    for index in 0..phnum {
        let ph = phoff + index * phentsize;
        assert!(ph + 56 <= image.len(), "truncated program header");
        if u32_at(image, ph) != PT_LOAD {
            continue;
        }
        let flags = u32_at(image, ph + 4);
        let offset = u64_at(image, ph + 8) as usize;
        let virtual_address = base + u64_at(image, ph + 16) as usize;
        let file_size = u64_at(image, ph + 32) as usize;
        let memory_size = u64_at(image, ph + 40) as usize;
        assert!(file_size <= memory_size && offset + file_size <= image.len());

        let mut pte_flags = PTE_U;
        if flags & PF_R != 0 {
            pte_flags |= PTE_R;
        }
        if flags & PF_W != 0 {
            pte_flags |= PTE_W;
        }
        if flags & PF_X != 0 {
            pte_flags |= PTE_X;
        }

        let first_page = virtual_address & !(PAGE_SIZE - 1);
        let last = virtual_address
            .checked_add(memory_size)
            .expect("ELF segment overflow");
        let mut page = first_page;
        while page < last {
            if table.translate(page).is_none() {
                table.map_page(page, alloc_frame(), pte_flags);
            } else {
                let physical = table.translate(page).unwrap() & !(PAGE_SIZE - 1);
                table.map_page(page, physical, pte_flags);
            }
            page += PAGE_SIZE;
        }

        for byte_index in 0..file_size {
            let destination = table.translate(virtual_address + byte_index).unwrap();
            unsafe { (destination as *mut u8).write(image[offset + byte_index]) };
        }
    }
    LoadedElf {
        entry,
        phdr: base + phoff,
        phent: phentsize,
        phnum,
    }
}
