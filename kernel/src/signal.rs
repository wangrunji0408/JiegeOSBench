//! Signals: handlers, masks, pending set, delivery via user stack sigframe,
//! and the rt_sigreturn trampoline.

use crate::task::TrapFrame;

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

pub const SA_NOCLDSTOP: u32 = 0x0000_0001;
pub const SA_NOCLDWAIT: u32 = 0x0000_0002;
pub const SA_SIGINFO: u32 = 0x0000_0004;
pub const SA_RESTORER: u32 = 0x0400_0000;
pub const SA_ONSTACK: u32 = 0x0800_0000;
pub const SA_RESTART: u32 = 0x1000_0000;
pub const SA_NODEFER: u32 = 0x4000_0000;
pub const SA_RESETHAND: u32 = 0x8000_0000;

pub const TRAMPOLINE: usize = 0x3fff_0000_0000;

#[derive(Clone)]
pub struct SignalState {
    pub handlers: [usize; 64],
    pub flags: [u32; 64],
    pub mask: u64,
    pub pending: u64,
    pub altstack_sp: usize,
    pub altstack_size: usize,
    pub altstack_active: bool,
}

impl SignalState {
    pub fn new() -> SignalState {
        SignalState {
            handlers: [SIG_DFL; 64],
            flags: [0; 64],
            mask: 0,
            pending: 0,
            altstack_sp: 0,
            altstack_size: 0,
            altstack_active: false,
        }
    }

    pub fn default_ignore(sig: usize) -> bool {
        matches!(sig, 17 | 20 | 28) // SIGCHLD, SIGTSTP? no: CHLD=17, URG=23, WINCH=28
    }
}

/// Returns the lowest pending, unmasked signal number, or 0.
pub fn next_pending() -> usize {
    let t = crate::task::current();
    let s = unsafe { &t.as_ref().unwrap().sig };
    let pending = s.pending & !s.mask;
    if pending == 0 {
        return 0;
    }
    (pending.trailing_zeros() as usize) + 1
}

pub fn has_pending() -> bool {
    next_pending() != 0
}

/// Deliver pending signals by rewriting the trapframe; may exit the task.
pub fn maybe_deliver(tf: *mut TrapFrame) {
    loop {
        let sig = next_pending();
        if sig == 0 {
            return;
        }
        let (handler, flags) = {
            let t = crate::task::current();
            let s = unsafe { &t.as_ref().unwrap().sig };
            (s.handlers[sig], s.flags[sig])
        };
        if handler == SIG_IGN {
            clear_pending(sig);
            continue;
        }
        if handler == SIG_DFL {
            // default action
            if SignalState::default_ignore(sig) {
                clear_pending(sig);
                continue;
            }
            // terminate (all remaining signals terminate by default)
            let t = crate::task::current();
            let t = unsafe { t.as_ref().unwrap() };
            crate::console::kprintln!("[sig] pid={} killed by SIG{}", t.pid, sig);
            crate::task::exit(128 + sig as i32);
        }
        // custom handler: build sigframe on user stack
        let (frame_addr, use_alt) = {
            let t = crate::task::current();
            let s = unsafe { &t.as_ref().unwrap().sig };
            if flags & SA_ONSTACK != 0 && s.altstack_size > 0 {
                (s.altstack_sp + s.altstack_size, true)
            } else {
                let sp = unsafe { (*tf).sp() };
                (sp, false)
            }
        };
        let frame_addr = frame_addr & !15;
        let frame_addr = frame_addr - 288;
        let tf = unsafe { &mut *tf };
        // save context into frame
        unsafe {
            let f = frame_addr as *mut usize;
            for i in 0..32 {
                *f.add(i) = tf.regs[i];
            }
            *f.add(32) = tf.sepc;
            *f.add(33) = tf.sstatus;
            *f.add(34) = {
                let t = crate::task::current();
                t.as_ref().unwrap().sig.mask
            };
            *f.add(35) = 0; // pad
            // set up handler call
            tf.regs[10] = sig; // a0
            tf.regs[1] = TRAMPOLINE; // ra
            tf.regs[2] = frame_addr + 288; // sp (16-aligned since 288%16==0)
            tf.sepc = handler;
            // mask
            let t = crate::task::current();
            let s = &mut t.as_ref().unwrap().sig;
            if flags & SA_NODEFER == 0 {
                s.mask |= (1u64 << (sig - 1)) | ((flags & SA_SIGINFO != 0) as u64 * 0);
            }
            if flags & SA_RESETHAND != 0 {
                s.handlers[sig] = SIG_DFL;
            }
            s.pending &= !(1u64 << sig);
            let _ = use_alt;
        }
        return;
    }
}

fn clear_pending(sig: usize) {
    let t = crate::task::current();
    unsafe {
        t.as_mut().unwrap().sig.pending &= !(1u64 << sig);
    }
}

/// Set a pending signal on a task and wake it if blocked.
pub fn send_signal(pid: usize, sig: usize) {
    if sig > 63 {
        return;
    }
    if let Some(t) = crate::task::task(pid) {
        if t.state == crate::task::TaskState::Zombie || t.state == crate::task::TaskState::Free {
            return;
        }
        t.sig.pending |= 1u64 << sig;
        if t.state == crate::task::TaskState::Blocked {
            t.state = crate::task::TaskState::Ready;
            t.wchan = 0;
            unsafe {
                crate::task::READY.push_back(pid);
            }
        }
    }
}

/// rt_sigreturn: restore context from the sigframe at user sp.
pub fn sigreturn(tf: *mut TrapFrame) {
    let sp = unsafe { (*tf).sp() };
    let f = sp as *const usize;
    unsafe {
        let t = crate::task::current();
        let out = &mut *tf;
        for i in 0..32 {
            out.regs[i] = *f.add(i);
        }
        out.sepc = *f.add(32);
        out.sstatus = *f.add(33);
        let mask = *f.add(34);
        t.as_mut().unwrap().sig.mask = mask;
    }
}

/// Map the rt_sigreturn trampoline page into a task's address space.
pub fn install_trampoline(mm: &mut crate::mm::vma::Mm) {
    let page = crate::mm::frame::alloc_frame().expect("trampoline");
    unsafe {
        // li a7, 139; ecall
        let code: [u8; 8] = [
            0x93, 0x08, 0x00, 0x00, // li a7, 0  (patched below)
            0x73, 0x00, 0x00, 0x00, // ecall
        ];
        // patch li a7, 139: 0x08b00893
        let instr = 0x08b0_0893u32.to_le_bytes();
        core::ptr::copy_nonoverlapping(instr.as_ptr(), page as *mut u8, 4);
        core::ptr::copy_nonoverlapping(code.as_ptr().add(4), (page + 4) as *mut u8, 4);
    }
    mm.pt.map(
        TRAMPOLINE,
        page,
        crate::mm::paging::PTE_R | crate::mm::paging::PTE_X | crate::mm::paging::PTE_U,
    );
    mm.vmas.push(crate::mm::vma::Vma {
        start: TRAMPOLINE,
        end: TRAMPOLINE + crate::mm::paging::PAGE_SIZE,
        prot: crate::mm::vma::PROT_READ | crate::mm::vma::PROT_EXEC,
        anon: true,
        file_id: None,
    });
}
