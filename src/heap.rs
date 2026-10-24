use linked_list_allocator::LockedHeap;

const HEAP_SIZE: usize = 16 * 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

#[unsafe(link_section = ".bss.heap")]
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

pub fn init() {
    unsafe {
        let start = (&raw mut HEAP).cast::<u8>();
        ALLOCATOR.lock().init(start, HEAP_SIZE);
    }
}
