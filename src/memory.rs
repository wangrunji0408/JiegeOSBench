use core::arch::asm;
static mut NEXT: usize = 0;
static mut ROOT: usize = 0;
pub static mut MMAP: usize = 0x10000000;
pub static mut BRK: usize = 0x8000000;
pub fn align(x: usize) -> usize {
    (x + 4095) & !4095
}
pub unsafe fn page() -> usize {
    let p = NEXT;
    NEXT += 4096;
    assert!(NEXT < 0x8f000000, "physical memory exhausted");
    core::ptr::write_bytes(p as *mut u8, 0, 4096);
    p
}
pub unsafe fn init() {
    extern "C" {
        static kernel_end: u8;
    }
    NEXT = align(core::ptr::addr_of!(kernel_end) as usize);
    ROOT = page();
    // Supervisor identity mappings for RAM; map device MMIO with 4 KiB leaves.
    (ROOT as *mut usize)
        .add(2)
        .write((0x80000000usize >> 12) << 10 | 0xcf);
    for p in (0x10000000..0x10010000).step_by(4096) {
        map_phys(p, p, 0xc7);
    }
    // User mmaps start elsewhere to avoid device range.
    MMAP = 0x12000000;
    asm!("csrw satp, {}",in(reg)(8usize<<60|ROOT>>12));
    asm!("sfence.vma","csrs sstatus, {}",in(reg)1usize<<18);
    crate::println!("[mm] Sv39 enabled, free physical memory at {:#x}", NEXT);
}
unsafe fn pte(va: usize) -> *mut usize {
    let mut table = ROOT;
    for level in (1..=2).rev() {
        let e = (table as *mut usize).add((va >> (12 + 9 * level)) & 511);
        if e.read() & 1 == 0 {
            e.write((page() >> 12) << 10 | 1);
        }
        table = (e.read() >> 10) << 12;
    }
    (table as *mut usize).add((va >> 12) & 511)
}
unsafe fn map_phys(va: usize, pa: usize, flags: usize) {
    pte(va).write((pa >> 12) << 10 | flags);
}
pub unsafe fn map(va: usize, len: usize) {
    if len == 0 {
        return;
    }
    assert!(va > 0 && va + len < 0x80000000);
    for p in (va & !4095..align(va + len)).step_by(4096) {
        let e = pte(p);
        if e.read() & 1 == 0 {
            e.write((page() >> 12) << 10 | 0xdf);
        }
    }
    asm!("sfence.vma");
}
pub unsafe fn alloc_map(len: usize) -> usize {
    let p = MMAP;
    MMAP += align(len);
    map(p, len);
    p
}
