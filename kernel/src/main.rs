#![no_std]
#![no_main]

extern crate alloc;

use core::arch::global_asm;
use core::panic::PanicInfo;

mod config;
mod console;
mod mm;
mod sbi;
mod trap;
mod lang_items {
    use super::*;

    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        crate::println!("[kernel] panicked: {}", info);
        crate::sbi::shutdown(true);
    }
}

global_asm!(include_str!("entry.asm"));

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    clear_bss();
    println!("[kernel] hello from riscv64 kernel!");
    trap::init();
    mm::init();
    println!("[kernel] paging enabled, kernel heap + frame allocator online");

    // Exercise the heap allocator (Vec/BTreeMap) to prove `alloc` works.
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;
    let mut v: Vec<i32> = Vec::new();
    for i in 0..2000 {
        v.push(i);
    }
    assert_eq!(v.iter().sum::<i32>(), (0..2000).sum());
    let mut m = BTreeMap::new();
    for i in 0..100 {
        m.insert(i, i * i);
    }
    assert_eq!(m[&50], 2500);
    println!("[kernel] heap allocator sanity check passed (Vec + BTreeMap)");

    // Exercise the frame allocator directly.
    let f1 = mm::frame_allocator::frame_alloc().unwrap();
    let f2 = mm::frame_allocator::frame_alloc().unwrap();
    println!(
        "[kernel] allocated frames ppn={:#x} ppn={:#x}",
        f1.ppn.0, f2.ppn.0
    );
    drop(f1);
    drop(f2);

    println!("[kernel] M2 memory management verified. shutting down.");
    sbi::shutdown(false);
}

fn clear_bss() {
    unsafe extern "C" {
        fn sbss();
        fn ebss();
    }
    unsafe {
        let start = sbss as *const () as usize;
        let end = ebss as *const () as usize;
        core::slice::from_raw_parts_mut(start as *mut u8, end - start).fill(0);
    }
}
