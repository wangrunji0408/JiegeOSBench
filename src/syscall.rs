use crate::arch::TrapFrame;

pub fn dispatch(frame: &mut TrapFrame) {
    let number = frame.regs[17];
    crate::println!("syscall {number} not implemented");
    frame.regs[10] = (-38isize) as usize;
}
