/// 保存在内核栈上的陷入上下文（用户寄存器快照）
#[repr(C)]
pub struct TrapContext {
    /// 通用寄存器 x0-x31
    pub x: [usize; 32],
    /// 用户态 sstatus
    pub sstatus: usize,
    /// 用户态 sepc（返回地址）
    pub sepc: usize,
    /// 保留字段（对齐）
    pub _reserved: [usize; 3],
}

impl TrapContext {
    pub fn sp(&self) -> usize { self.x[2] }
    pub fn set_sp(&mut self, sp: usize) { self.x[2] = sp; }
    pub fn a0(&self) -> usize { self.x[10] }
    pub fn set_a0(&mut self, val: usize) { self.x[10] = val; }
    pub fn syscall_id(&self) -> usize { self.x[17] } // a7
    pub fn args(&self) -> [usize; 6] {
        [self.x[10], self.x[11], self.x[12], self.x[13], self.x[14], self.x[15]]
    }

    /// 为新进程创建初始陷入上下文
    pub fn new_user(entry: usize, sp: usize) -> Self {
        // 设置sstatus：SPP=User, SPIE=1（开启用户态中断）
        let sstatus = {
            // SPP bit[8] = 0 (User), SPIE bit[5] = 1
            let mut val: usize;
            unsafe {
                core::arch::asm!("csrr {}, sstatus", out(reg) val);
            }
            val &= !(1 << 8); // SPP = 0 (User)
            val |= 1 << 5;    // SPIE = 1
            val &= !(1 << 1); // SIE = 0
            val
        };
        let mut cx = Self {
            x: [0; 32],
            sstatus,
            sepc: entry,
            _reserved: [0; 3],
        };
        cx.set_sp(sp);
        cx
    }
}

/// 内核任务上下文（用于任务切换）
/// 布局必须与__switch汇编匹配：ra, sp, s0-s11
#[repr(C)]
pub struct TaskContext {
    /// ra: 切换回来后的返回地址
    pub ra: usize,
    /// sp: 内核栈指针
    pub sp: usize,
    /// s0-s11: callee-saved寄存器
    pub s: [usize; 12],
}

impl TaskContext {
    pub fn zero() -> Self {
        Self { ra: 0, sp: 0, s: [0; 12] }
    }

    /// 创建一个会在切换后执行__restore的上下文
    pub fn goto_restore(kernel_sp: usize) -> Self {
        extern "C" { fn __restore(); }
        Self {
            ra: __restore as usize,
            sp: kernel_sp,
            s: [0; 12],
        }
    }
}

extern "C" {
    pub fn __switch(current: *mut TaskContext, next: *const TaskContext);
}
