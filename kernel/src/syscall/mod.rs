//! Syscall dispatch. Numbers follow the generic riscv64/arm64 Linux ABI
//! (`include/uapi/asm-generic/unistd.h`), confirmed against a real strace
//! of nginx running under `qemu-riscv64 -strace`.

mod fs;
mod process;

const SYSCALL_WRITE: usize = 64;
const SYSCALL_EXIT: usize = 93;
const SYSCALL_EXIT_GROUP: usize = 94;

pub fn syscall(id: usize, args: [usize; 6]) -> isize {
    match id {
        SYSCALL_WRITE => fs::sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_EXIT | SYSCALL_EXIT_GROUP => process::sys_exit(args[0] as i32),
        _ => {
            crate::println!("[kernel] unsupported syscall id={}, args={:?}", id, args);
            -38 // ENOSYS
        }
    }
}
