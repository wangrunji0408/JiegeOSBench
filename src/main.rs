#![no_std]
#![no_main]

mod console;
mod elf;
mod fs;
mod memory;
mod network;
mod sbi;
mod trap;

use core::arch::global_asm;
use core::panic::PanicInfo;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    call rust_main
1:  wfi
    j 1b

    .section .bss.stack
    .align 12
boot_stack:
    .space 65536
boot_stack_top:
"#
);

unsafe extern "C" {
    static mut sbss: u8;
    static mut ebss: u8;
    static skernel: u8;
    static ekernel: u8;
}

#[unsafe(no_mangle)]
extern "C" fn rust_main(hart_id: usize, device_tree: usize) -> ! {
    unsafe {
        let start = core::ptr::addr_of_mut!(sbss);
        let len = core::ptr::addr_of_mut!(ebss).offset_from(start) as usize;
        core::ptr::write_bytes(start, 0, len);
    }

    println!("\nJiege kernel 0.1.0");
    println!("hart={} dtb={:#x}", hart_id, device_tree);
    println!(
        "kernel=[{:#x}, {:#x})",
        core::ptr::addr_of!(skernel) as usize,
        core::ptr::addr_of!(ekernel) as usize
    );
    memory::init();
    network::init();
    fs::init();
    trap::init();

    let page_table = memory::PageTable::new();
    page_table.map_kernel();
    let loader = fs::file(b"/lib/ld-musl-riscv64.so.1").expect("musl loader missing");
    const LOADER_BASE: usize = 0x4000_0000;
    let loaded = elf::load_at(loader, page_table, LOADER_BASE);
    const STACK_PAGES: usize = 16;
    for page in 1..=STACK_PAGES {
        page_table.map_page(
            trap::USER_STACK_TOP - page * memory::PAGE_SIZE,
            memory::alloc_frame(),
            memory::PTE_U | memory::PTE_R | memory::PTE_W,
        );
    }
    let satp = page_table.activate();
    let stack = prepare_initial_stack(&loaded);
    println!(
        "entering musl: entry={:#x} nginx=/usr/sbin/nginx",
        loaded.entry
    );
    trap::enter(loaded.entry, stack, satp)
}

fn prepare_initial_stack(loaded: &elf::LoadedElf) -> usize {
    const ARGUMENTS: [&[u8]; 2] = [b"/lib/ld-musl-riscv64.so.1\0", b"/usr/sbin/nginx\0"];
    const ENVIRONMENT: [&[u8]; 2] = [
        b"PATH=/usr/sbin:/usr/bin:/sbin:/bin\0",
        b"LD_LIBRARY_PATH=/usr/lib:/lib\0",
    ];

    let mut cursor = trap::USER_STACK_TOP;
    let mut argv = [0usize; ARGUMENTS.len()];
    let mut envp = [0usize; ENVIRONMENT.len()];
    for (index, string) in ARGUMENTS.iter().enumerate().rev() {
        cursor -= string.len();
        argv[index] = cursor;
        for (offset, byte) in string.iter().enumerate() {
            assert!(memory::write_user_byte(cursor + offset, *byte));
        }
    }
    for (index, string) in ENVIRONMENT.iter().enumerate().rev() {
        cursor -= string.len();
        envp[index] = cursor;
        for (offset, byte) in string.iter().enumerate() {
            assert!(memory::write_user_byte(cursor + offset, *byte));
        }
    }
    cursor -= 16;
    let random = cursor;
    for index in 0..16 {
        assert!(memory::write_user_byte(
            random + index,
            (0x5a ^ index as u8).wrapping_mul(17)
        ));
    }

    let auxv = [
        (3usize, loaded.phdr),
        (4, loaded.phent),
        (5, loaded.phnum),
        (6, memory::PAGE_SIZE),
        (7, 0),
        (8, 0),
        (9, loaded.entry),
        (11, 0),
        (12, 0),
        (13, 0),
        (14, 0),
        (23, 0),
        (25, random),
        (31, argv[0]),
        (0, 0),
    ];
    let word_count = 1 + argv.len() + 1 + envp.len() + 1 + auxv.len() * 2;
    cursor = (cursor - word_count * core::mem::size_of::<usize>()) & !15;
    let mut word = cursor;
    let push = |value: usize, word: &mut usize| {
        assert!(memory::write_user_usize(*word, value));
        *word += core::mem::size_of::<usize>();
    };
    push(argv.len(), &mut word);
    for value in argv {
        push(value, &mut word);
    }
    push(0, &mut word);
    for value in envp {
        push(value, &mut word);
    }
    push(0, &mut word);
    for (kind, value) in auxv {
        push(kind, &mut word);
        push(value, &mut word);
    }
    cursor
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    println!("\nKERNEL PANIC: {info}");
    sbi::shutdown(true)
}
