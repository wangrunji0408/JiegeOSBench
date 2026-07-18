//! 信号相关类型（最小实现：保存 sigaction，不做投递）

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct SigAction {
    pub handler: usize,
    pub flags: usize,
    pub restorer: usize,
    pub mask: u64,
}

pub const SIGCHLD: usize = 17;
pub const SIGKILL: usize = 9;
pub const SIGTERM: usize = 15;
