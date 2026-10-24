use spin::Mutex;

use crate::elf::ProcessImage;

pub struct ProcessState {
    pub image: ProcessImage,
    pub program_break_base: usize,
    pub program_break: usize,
    pub mmap_next: usize,
}

static PROCESS: Mutex<Option<ProcessState>> = Mutex::new(None);

pub fn start() -> ! {
    let image = crate::elf::load(
        "/usr/sbin/nginx",
        &["nginx", "-c", "/etc/nginx/nginx.conf"],
        &[
            "PATH=/usr/sbin:/usr/bin:/sbin:/bin",
            "HOME=/var/lib/nginx",
            "TZ=UTC",
        ],
    )
    .unwrap_or_else(|error| panic!("cannot load nginx: {error:?}"));
    let program_break = image.program_break;
    let mmap_next = image.mmap_next;
    *PROCESS.lock() = Some(ProcessState {
        image,
        program_break_base: program_break,
        program_break,
        mmap_next,
    });
    crate::println!("process: entering official nginx in U-mode");
    let process = PROCESS.lock();
    let process = process.as_ref().unwrap();
    crate::arch::enter_user(process.image.entry, process.image.stack)
}

pub fn with_state<R>(function: impl FnOnce(&mut ProcessState) -> R) -> R {
    let mut process = PROCESS.lock();
    function(process.as_mut().expect("process not initialized"))
}
