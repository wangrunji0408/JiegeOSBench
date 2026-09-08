use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::csr::disable_interrupts();
    crate::console::write_str_raw("\n!!! kernel panic: ");
    if let Some(loc) = info.location() {
        crate::print!("{}:{}: ", loc.file(), loc.line());
    }
    crate::print!("{}\n", info.message());
    crate::sbi::shutdown(true);
}

#[no_mangle]
pub extern "Rust" fn __rust_alloc_error_handler(size: usize, _align: usize) -> ! {
    crate::println!("out of memory (allocation of {} bytes failed)", size);
    crate::sbi::shutdown(true);
}
