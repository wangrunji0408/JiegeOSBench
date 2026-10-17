use crate::arch::{self, TrapFrame};
use crate::println;

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let scause = arch::scause();
    if scause & (1 << 63) != 0 {
        // interrupt: nothing enabled yet
        return;
    }
    let code = scause & 0xff;
    match code {
        8 => {
            crate::syscall::dispatch(tf);
            tf.sepc += 4;
        }
        9 => {
            crate::syscall::dispatch(tf);
            tf.sepc += 4;
        }
        _ => {
            println!(
                "[trap] fatal exception code={} sepc={:#x} stval={:#x} sp={:#x}",
                code,
                arch::sepc(),
                arch::stval(),
                tf.x[2]
            );
            crate::sbi::shutdown(true);
        }
    }
}
