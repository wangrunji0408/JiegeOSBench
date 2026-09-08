//! POSIX signals: registration, delivery (Linux riscv64 rt_sigframe layout) and
//! rt_sigreturn.

use crate::errno::*;
use crate::task::process::Process;
use crate::trap::TrapFrame;

pub const SIGHUP: usize = 1;
pub const SIGINT: usize = 2;
pub const SIGQUIT: usize = 3;
pub const SIGILL: usize = 4;
pub const SIGTRAP: usize = 5;
pub const SIGABRT: usize = 6;
pub const SIGBUS: usize = 7;
pub const SIGFPE: usize = 8;
pub const SIGKILL: usize = 9;
pub const SIGUSR1: usize = 10;
pub const SIGSEGV: usize = 11;
pub const SIGUSR2: usize = 12;
pub const SIGPIPE: usize = 13;
pub const SIGALRM: usize = 14;
pub const SIGTERM: usize = 15;
pub const SIGCHLD: usize = 17;
pub const SIGCONT: usize = 18;
pub const SIGSTOP: usize = 19;
pub const SIGTSTP: usize = 20;
pub const SIGTTIN: usize = 21;
pub const SIGTTOU: usize = 22;
pub const SIGURG: usize = 23;
pub const SIGXCPU: usize = 24;
pub const SIGXFSZ: usize = 25;
pub const SIGVTALRM: usize = 26;
pub const SIGPROF: usize = 27;
pub const SIGWINCH: usize = 28;
pub const SIGIO: usize = 29;
pub const SIGPWR: usize = 30;
pub const SIGSYS: usize = 31;

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

pub const SA_SIGINFO: u64 = 0x0000_0004;
pub const SA_ONSTACK: u64 = 0x0800_0000;
pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;
pub const SA_RESTORER: u64 = 0x0400_0000;

/// sizeof(struct rt_sigframe) rounded to 16, for riscv64.
pub const SIGFRAME_SIZE: usize = 1088;
const UC_OFF: usize = 128; // ucontext inside the frame
const UC_MCONTEXT: usize = 168; // mcontext inside ucontext

#[derive(Clone, Copy, Debug)]
pub struct SigAction {
    pub handler: usize,
    pub flags: u64,
    pub restorer: usize,
    pub mask: u64,
}

impl Default for SigAction {
    fn default() -> Self {
        Self {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        }
    }
}

#[derive(Clone)]
pub struct SignalState {
    pub actions: [SigAction; 65],
    pub pending: u64,
    pub blocked: u64,
    pub altstack_sp: usize,
    pub altstack_size: usize,
    pub altstack_flags: u32,
    pub info: [SigInfo; 65],
    pub in_sigsuspend: bool,
    pub suspended_mask: u64,
}

#[derive(Clone, Copy, Default)]
pub struct SigInfo {
    pub code: i32,
    pub pid: i32,
    pub uid: u32,
    pub status: i32,
    pub addr: usize,
    pub has_info: bool,
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            actions: [SigAction::default(); 65],
            pending: 0,
            blocked: 0,
            altstack_sp: 0,
            altstack_size: 0,
            altstack_flags: 2, // SS_DISABLE
            info: [SigInfo::default(); 65],
            in_sigsuspend: false,
            suspended_mask: 0,
        }
    }

    pub fn reset_for_exec(&mut self) {
        for a in self.actions.iter_mut() {
            if a.handler != SIG_IGN {
                *a = SigAction::default();
            } else {
                a.handler = SIG_IGN;
                a.flags = 0;
                a.mask = 0;
                a.restorer = 0;
            }
        }
        self.pending = 0;
        self.blocked = 0;
        self.altstack_flags = 2;
        self.altstack_size = 0;
        self.altstack_sp = 0;
    }

    pub fn raise(&mut self, sig: usize) {
        if sig > 0 && sig < 65 {
            self.pending |= 1u64 << (sig - 1);
        }
    }

    pub fn raise_with(&mut self, sig: usize, info: SigInfo) {
        if sig > 0 && sig < 65 {
            self.pending |= 1u64 << (sig - 1);
            self.info[sig] = info;
            self.info[sig].has_info = true;
        }
    }

    pub fn is_ignored(&self, sig: usize) -> bool {
        self.actions[sig].handler == SIG_IGN
    }

    pub fn is_default(&self, sig: usize) -> bool {
        self.actions[sig].handler == SIG_DFL
    }

    fn next_deliverable(&self) -> Option<usize> {
        let deliverable = self.pending & !self.blocked & !(1u64 << (SIGKILL - 1)) & !(1u64 << (SIGSTOP - 1));
        if deliverable == 0 {
            return None;
        }
        Some(deliverable.trailing_zeros() as usize + 1)
    }
}

fn default_action_is_ignore(sig: usize) -> bool {
    matches!(
        sig,
        SIGCHLD | SIGURG | SIGWINCH | SIGCONT | SIGIO
    )
}

fn default_is_stop(sig: usize) -> bool {
    matches!(sig, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU)
}

/// Deliver one pending signal to the process. Returns true if a handler frame was built.
pub fn deliver(proc: &mut Process, tf: &mut TrapFrame) -> bool {
    let sig = match proc.sig.next_deliverable() {
        Some(s) => s,
        None => return false,
    };
    // Signals that were only queued for a blocked mask are cleared once consumed.
    proc.sig.pending &= !(1u64 << (sig - 1));
    if proc.sig.in_sigsuspend {
        // rt_sigsuspend: restore the original mask now that a signal arrived
        proc.sig.blocked = proc.sig.suspended_mask;
        proc.sig.in_sigsuspend = false;
        tf.sepc = tf.sepc.wrapping_sub(4); // re-execute the ecall? no: set EINTR
        // The syscall will return EINTR through the normal path; leave sepc.
    }

    let action = proc.sig.actions[sig];
    if action.handler == SIG_IGN {
        return false;
    }
    if action.handler == SIG_DFL {
        if default_action_is_ignore(sig) || default_is_stop(sig) {
            return false;
        }
        crate::println!("[sig] pid {} terminated by signal {}", proc.pid, sig);
        proc.exit_code = 128 + sig as i32;
        proc.exited = true;
        return false;
    }

    // Build the signal frame on the user stack.
    let mut sp = tf.sp();
    if action.flags & SA_ONSTACK != 0 && proc.sig.altstack_flags & 1 == 0 && proc.sig.altstack_sp != 0 {
        sp = proc.sig.altstack_sp + proc.sig.altstack_size;
    }
    let frame = (sp - SIGFRAME_SIZE) & !15;
    let mut frame_data = alloc::vec![0u8; SIGFRAME_SIZE];
    let info = proc.sig.info[sig];
    // siginfo
    frame_data[0..4].copy_from_slice(&(sig as i32).to_le_bytes());
    frame_data[4..8].copy_from_slice(&0i32.to_le_bytes());
    let code = if info.has_info {
        info.code
    } else {
        0x80 // SI_KERNEL
    };
    frame_data[8..12].copy_from_slice(&code.to_le_bytes());
    if sig == SIGCHLD {
        frame_data[16..20].copy_from_slice(&info.pid.to_le_bytes());
        frame_data[20..24].copy_from_slice(&info.uid.to_le_bytes());
        frame_data[24..28].copy_from_slice(&info.status.to_le_bytes());
    } else if sig == SIGSEGV || sig == SIGBUS || sig == SIGILL || sig == SIGFPE {
        frame_data[16..24].copy_from_slice(&(info.addr as u64).to_le_bytes());
    }
    // ucontext
    let uc = UC_OFF;
    frame_data[uc + 16..uc + 24].copy_from_slice(&(proc.sig.altstack_sp as u64).to_le_bytes());
    frame_data[uc + 24..uc + 28].copy_from_slice(&proc.sig.altstack_flags.to_le_bytes());
    frame_data[uc + 32..uc + 40].copy_from_slice(&(proc.sig.altstack_size as u64).to_le_bytes());
    frame_data[uc + 40..uc + 48].copy_from_slice(&proc.sig.blocked.to_le_bytes());
    // gregs: pc, ra, sp, gp, tp, t0..t6, s0, s1, a0..a7, s2..s11, t3..t6
    let gregs = uc + UC_MCONTEXT;
    let order: [usize; 32] = [
        0, // placeholder for pc
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31,
    ];
    frame_data[gregs..gregs + 8].copy_from_slice(&(tf.sepc as u64).to_le_bytes());
    for i in 1..32 {
        let off = gregs + i * 8;
        frame_data[off..off + 8].copy_from_slice(&(tf.x[order[i]] as u64).to_le_bytes());
    }
    // fp state
    unsafe {
        __fp_save(frame_data[gregs + 256..].as_mut_ptr());
    }
    if proc.copy_to_user(frame, &frame_data).is_err() {
        return false;
    }

    // block the signal and its mask
    proc.sig.blocked |= action.mask;
    if action.flags & SA_NODEFER == 0 {
        proc.sig.blocked |= 1u64 << (sig - 1);
    }
    if action.flags & SA_RESETHAND != 0 {
        proc.sig.actions[sig] = SigAction::default();
    }

    tf.sepc = action.handler;
    tf.x[2] = frame;
    tf.x[10] = sig;
    tf.x[11] = frame + 0; // siginfo
    tf.x[12] = frame + UC_OFF; // ucontext
    tf.x[1] = crate::task::process::SIGRETURN_TRAMPOLINE;
    true
}

/// rt_sigreturn: restore the context saved in the frame at tf.sp.
pub fn sigreturn(proc: &mut Process, tf: &mut TrapFrame) -> Result<usize, Errno> {
    let frame = tf.sp();
    let mut data = alloc::vec![0u8; SIGFRAME_SIZE];
    proc.copy_from_user(frame, &mut data)?;
    let uc = UC_OFF;
    let mask = u64::from_le_bytes(data[uc + 40..uc + 48].try_into().unwrap());
    let alt_sp = u64::from_le_bytes(data[uc + 16..uc + 24].try_into().unwrap()) as usize;
    let alt_flags = u32::from_le_bytes(data[uc + 24..uc + 28].try_into().unwrap());
    let alt_size = u64::from_le_bytes(data[uc + 32..uc + 40].try_into().unwrap()) as usize;
    let gregs = uc + UC_MCONTEXT;
    tf.sepc = u64::from_le_bytes(data[gregs..gregs + 8].try_into().unwrap()) as usize;
    for i in 1..32 {
        let off = gregs + i * 8;
        tf.x[i] = u64::from_le_bytes(data[off..off + 8].try_into().unwrap()) as usize;
    }
    unsafe {
        __fp_restore(data[gregs + 256..].as_mut_ptr());
    }
    proc.sig.blocked = mask;
    proc.sig.altstack_sp = alt_sp;
    proc.sig.altstack_flags = alt_flags;
    proc.sig.altstack_size = alt_size;
    Ok(tf.x[10])
}

extern "C" {
    fn __fp_save(buf: *mut u8);
    fn __fp_restore(buf: *const u8);
}
