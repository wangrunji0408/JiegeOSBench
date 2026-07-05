//! 调度器（占位）。Phase 4 实现真正的多任务调度。

use crate::trap::TrapContext;

/// 时钟中断时调用。当前无任务可调度，直接返回。
pub fn on_tick() {}

/// 当前进程退出。此阶段无进程，直接关机。
pub fn exit_current(code: i32) -> ! {
    crate::println!("[sched] exit code={}", code);
    crate::sbi::shutdown();
}
