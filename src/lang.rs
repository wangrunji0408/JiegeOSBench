//! Kernel-side panic handling and language items.

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use crate::console::print_fmt;
    print_fmt(format_args!("\n\x1b[31m[KERNEL PANIC]\x1b[0m\n"));
    if let Some(loc) = info.location() {
        print_fmt(format_args!("  at {}:{}:{}\n", loc.file(), loc.line(), loc.column()));
    }
    print_fmt(format_args!("  {}\n", info.message()));
    crate::sbi::shutdown();
    loop {}
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("heap allocation failed: {:?}", layout);
}
