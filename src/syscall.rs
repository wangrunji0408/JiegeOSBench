//! 系统调用分发（Linux RISC-V ABI）。
//! Phase 6 会大幅扩展；此阶段提供 write 占位以让 trap 路径可编译。

use crate::trap::TrapContext;

#[no_mangle]
pub fn do_syscall(cx: &mut TrapContext) {
    let num = cx.x[17]; // a7 = syscall number
    match num {
        // Linux: write(fd, buf, count)
        64 => {
            let fd = cx.x[10];
            let buf = cx.x[11] as *const u8;
            let count = cx.x[12];
            // 此阶段直接经 UART 输出（无用户地址空间校验）
            if fd == 1 || fd == 2 {
                for i in 0..count {
                    let c = unsafe { core::ptr::read_volatile(buf.add(i)) };
                    crate::uart::putc(c);
                }
                cx.x[10] = count;
            } else {
                cx.x[10] = (-9isize) as usize; // EBADF
            }
        }
        // exit / exit_group
        93 | 94 => {
            crate::println!("[syscall] process exited with code {}", cx.x[10]);
            crate::sched::exit_current(cx.x[10] as i32);
        }
        // getpid
        172 => {
            cx.x[10] = crate::sched::current_pid();
        }
        _ => {
            crate::println!("[syscall] unsupported num {} (a0={:#x})", num, cx.x[10]);
            cx.x[10] = (-38isize) as usize; // ENOSYS
        }
    }
}
