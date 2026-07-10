use core::sync::atomic::{AtomicUsize, Ordering};

pub const PAGE_SIZE: usize = 4096;
const MEMORY_END: usize = 0x8f00_0000;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;

static NEXT_FRAME: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_ROOT: AtomicUsize = AtomicUsize::new(0);
static NEXT_MMAP: AtomicUsize = AtomicUsize::new(0x1_0000_0000);

unsafe extern "C" {
    static ekernel: u8;
}

#[inline]
const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

pub fn init() {
    let first = align_up(core::ptr::addr_of!(ekernel) as usize, PAGE_SIZE);
    NEXT_FRAME.store(first, Ordering::SeqCst);
    crate::println!("physical frames: [{:#x}, {:#x})", first, MEMORY_END);
}

pub fn alloc_frame() -> usize {
    let frame = NEXT_FRAME.fetch_add(PAGE_SIZE, Ordering::SeqCst);
    assert!(frame + PAGE_SIZE <= MEMORY_END, "out of physical memory");
    unsafe { core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE) };
    frame
}

#[derive(Clone, Copy)]
pub struct PageTable {
    root: usize,
}

impl PageTable {
    pub fn new() -> Self {
        Self {
            root: alloc_frame(),
        }
    }

    fn table_entry(table: usize, index: usize) -> *mut usize {
        (table + index * core::mem::size_of::<usize>()) as *mut usize
    }

    pub fn map_page(self, virtual_address: usize, physical_address: usize, flags: usize) {
        self.map_page_inner(virtual_address, physical_address, flags, false);
    }

    pub fn replace_page(self, virtual_address: usize, physical_address: usize, flags: usize) {
        self.map_page_inner(virtual_address, physical_address, flags, true);
    }

    fn map_page_inner(
        self,
        virtual_address: usize,
        physical_address: usize,
        flags: usize,
        replace: bool,
    ) {
        assert_eq!(virtual_address & (PAGE_SIZE - 1), 0);
        assert_eq!(physical_address & (PAGE_SIZE - 1), 0);
        let indexes = [
            (virtual_address >> 12) & 0x1ff,
            (virtual_address >> 21) & 0x1ff,
            (virtual_address >> 30) & 0x1ff,
        ];
        let mut table = self.root;
        for level in (1..=2).rev() {
            let entry = Self::table_entry(table, indexes[level]);
            let mut value = unsafe { entry.read_volatile() };
            if value & PTE_V == 0 {
                let child = alloc_frame();
                value = ((child >> 12) << 10) | PTE_V;
                unsafe { entry.write_volatile(value) };
            }
            assert_eq!(
                value & (PTE_R | PTE_W | PTE_X),
                0,
                "mapping crosses a huge page"
            );
            table = ((value >> 10) << 12) as usize;
        }
        let leaf = Self::table_entry(table, indexes[0]);
        let old = unsafe { leaf.read_volatile() };
        if old & PTE_V != 0 && !replace {
            let old_pa = ((old >> 10) << 12) as usize;
            assert_eq!(old_pa, physical_address, "virtual page remapped");
            unsafe { leaf.write_volatile(old | flags | PTE_A | PTE_D | PTE_V) };
        } else {
            let value = ((physical_address >> 12) << 10) | flags | PTE_A | PTE_D | PTE_V;
            unsafe { leaf.write_volatile(value) };
        }
    }

    fn map_huge_2m(self, address: usize, flags: usize) {
        assert_eq!(address & ((1 << 21) - 1), 0);
        let root_index = (address >> 30) & 0x1ff;
        let mid_index = (address >> 21) & 0x1ff;
        let root_entry = Self::table_entry(self.root, root_index);
        let mut root_value = unsafe { root_entry.read_volatile() };
        if root_value & PTE_V == 0 {
            let child = alloc_frame();
            root_value = ((child >> 12) << 10) | PTE_V;
            unsafe { root_entry.write_volatile(root_value) };
        }
        let middle = ((root_value >> 10) << 12) as usize;
        let leaf = Self::table_entry(middle, mid_index);
        assert_eq!(unsafe { leaf.read_volatile() } & PTE_V, 0);
        let value = ((address >> 12) << 10) | flags | PTE_A | PTE_D | PTE_V;
        unsafe { leaf.write_volatile(value) };
    }

    pub fn map_kernel(self) {
        self.map_huge_2m(0x1000_0000, PTE_R | PTE_W | PTE_G);
        let mut address = 0x8000_0000;
        while address < 0x9000_0000 {
            self.map_huge_2m(address, PTE_R | PTE_W | PTE_X | PTE_G);
            address += 1 << 21;
        }
    }

    pub fn translate(self, virtual_address: usize) -> Option<usize> {
        let indexes = [
            (virtual_address >> 12) & 0x1ff,
            (virtual_address >> 21) & 0x1ff,
            (virtual_address >> 30) & 0x1ff,
        ];
        let mut table = self.root;
        for level in (0..=2).rev() {
            let value = unsafe { Self::table_entry(table, indexes[level]).read_volatile() };
            if value & PTE_V == 0 {
                return None;
            }
            if value & (PTE_R | PTE_X) != 0 {
                let page_bits = 12 + 9 * level;
                let mask = (1usize << page_bits) - 1;
                return Some((((value >> 10) << 12) & !mask) | (virtual_address & mask));
            }
            table = ((value >> 10) << 12) as usize;
        }
        None
    }

    pub fn activate(self) -> usize {
        ACTIVE_ROOT.store(self.root, Ordering::SeqCst);
        (8usize << 60) | (self.root >> 12)
    }

    pub fn map_user_memory(self, address: usize, length: usize, flags: usize) {
        self.map_user_memory_inner(address, length, flags, false);
    }

    pub fn replace_user_memory(self, address: usize, length: usize, flags: usize) {
        self.map_user_memory_inner(address, length, flags, true);
    }

    fn map_user_memory_inner(self, address: usize, length: usize, flags: usize, replace: bool) {
        let start = address & !(PAGE_SIZE - 1);
        let end = align_up(address.saturating_add(length), PAGE_SIZE);
        let mut page = start;
        while page < end {
            if self.translate(page).is_none() || replace {
                let frame = alloc_frame();
                if replace {
                    self.replace_page(page, frame, flags | PTE_U);
                } else {
                    self.map_page(page, frame, flags | PTE_U);
                }
            } else {
                let physical = self.translate(page).unwrap() & !(PAGE_SIZE - 1);
                self.map_page(page, physical, flags | PTE_U);
            }
            page += PAGE_SIZE;
        }
    }
}

pub fn active_page_table() -> PageTable {
    PageTable {
        root: ACTIVE_ROOT.load(Ordering::SeqCst),
    }
}

pub fn read_user_byte(address: usize) -> Option<u8> {
    let physical = active_page_table().translate(address)?;
    Some(unsafe { (physical as *const u8).read_volatile() })
}

pub fn write_user_byte(address: usize, byte: u8) -> bool {
    let Some(physical) = active_page_table().translate(address) else {
        return false;
    };
    unsafe { (physical as *mut u8).write_volatile(byte) };
    true
}

pub fn write_user_usize(address: usize, value: usize) -> bool {
    for (index, byte) in value.to_le_bytes().iter().enumerate() {
        if !write_user_byte(address + index, *byte) {
            return false;
        }
    }
    true
}

pub fn read_user_usize(address: usize) -> Option<usize> {
    let mut bytes = [0u8; core::mem::size_of::<usize>()];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = read_user_byte(address + index)?;
    }
    Some(usize::from_le_bytes(bytes))
}

pub fn zero_user(address: usize, length: usize) -> bool {
    for offset in 0..length {
        if !write_user_byte(address + offset, 0) {
            return false;
        }
    }
    true
}

pub fn allocate_mmap_address(length: usize) -> usize {
    let length = align_up(length, PAGE_SIZE);
    NEXT_MMAP.fetch_add(length + PAGE_SIZE, Ordering::SeqCst)
}
