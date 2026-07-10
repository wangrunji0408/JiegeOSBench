/// 调度器
/// 协作式+时间片轮转调度

use crate::arch::context::{TaskContext, __switch};
use super::manager::TASK_MANAGER;
use super::process::TaskState;

/// 空闲任务的上下文（当没有任务可运行时使用）
static mut IDLE_CONTEXT: TaskContext = TaskContext { ra: 0, sp: 0, s: [0; 12] };

/// 启动调度器（不返回）
pub fn run() -> ! {
    // 初始化IDLE上下文
    unsafe {
        IDLE_CONTEXT.ra = idle_task as usize;
        // idle任务使用boot stack
        extern "C" { fn boot_stack_top(); }
        IDLE_CONTEXT.sp = boot_stack_top as usize;
    }

    // 开始第一次调度
    schedule();

    unreachable!("scheduler returned")
}

/// 切换到下一个任务（可能从中断/syscall中调用）
pub fn schedule() {
    let (next_pid, current_pid_opt) = {
        let mut mgr = TASK_MANAGER.lock();
        // 将当前任务放回就绪队列
        let cur = mgr.current;
        if let Some(pid) = cur {
            let state = mgr.tasks.get(&pid).map(|t| t.lock().state);
            if state == Some(TaskState::Running) {
                mgr.tasks.get(&pid).map(|t| t.lock().state = TaskState::Ready);
                mgr.ready_queue.push_back(pid);
            }
        }
        (mgr.pick_next(), cur)
    };

    if let Some(next_pid) = next_pid {
        // If rescheduling the SAME task, just return without corrupting TaskContext
        if Some(next_pid) == current_pid_opt {
            let mut mgr = TASK_MANAGER.lock();
            if let Some(task) = mgr.tasks.get(&next_pid) {
                task.lock().state = TaskState::Running;
            }
            mgr.current = Some(next_pid);
            return;
        }
        switch_to(next_pid);
    } else {
        // 没有任务可运行，轮询网络
        crate::net::poll();
        // 确保sscratch=0（内核态标记）
        unsafe { riscv::register::sscratch::write(0); }
        // 开启中断并等待
        unsafe {
            riscv::register::sstatus::set_sie();
            core::arch::asm!("wfi");
            riscv::register::sstatus::clear_sie();
        }
    }
}

fn switch_to(next_pid: usize) {
    let (next_task_ctx_ptr, current_task_ctx_ptr) = {
        let mut mgr = TASK_MANAGER.lock();

        // 设置下一个任务为运行
        let next = mgr.tasks.get(&next_pid).unwrap().clone();
        next.lock().state = TaskState::Running;

        // 设置内核栈指针（用于trap）
        let next_ksp = next.lock().kernel_stack_top;
        crate::arch::trap::set_kernel_stack(next_ksp);

        // 注意：不再在这里激活用户页表！
        // 用户页表切换由trap.S的.restore_user处理（从TrapContext的slot 34读取user satp）
        // 我们只需要确保内核代码运行在KERNEL_SPACE下
        // KERNEL_SPACE由trap.S的from_user路径切换，当前应该已经是KERNEL_SPACE了

        let current_pid = mgr.current.take();
        mgr.current = Some(next_pid);

        // 获取上下文指针
        let next_ctx = {
            let task = next.lock();
            &task.task_context as *const TaskContext
        };

        // 如果有当前任务，保存其上下文
        if let Some(cur_pid) = current_pid {
            if let Some(cur_task) = mgr.tasks.get(&cur_pid) {
                let cur_ctx = {
                    let task = cur_task.lock();
                    (&task.task_context as *const TaskContext) as *mut TaskContext
                };
                (next_ctx, cur_ctx)
            } else {
                // 当前任务已退出
                (next_ctx, unsafe { &mut IDLE_CONTEXT as *mut TaskContext })
            }
        } else {
            (next_ctx, unsafe { &mut IDLE_CONTEXT as *mut TaskContext })
        }
    };

    unsafe {
        __switch(current_task_ctx_ptr, next_task_ctx_ptr);
    }
}

/// 空闲任务：循环等待中断
fn idle_task() -> ! {
    loop {
        crate::net::poll();
        unsafe {
            riscv::register::sstatus::set_sie();
            core::arch::asm!("wfi");
        }
        schedule();
    }
}
