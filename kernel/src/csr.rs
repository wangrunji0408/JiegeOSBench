//! CSR access helpers and bit definitions.

macro_rules! csrr {
    ($csr:literal) => {{
        let r: usize;
        unsafe {
            core::arch::asm!(concat!("csrr {0}, ", $csr), out(reg) r, options(nomem, nostack));
        }
        r
    }};
}

macro_rules! csrw {
    ($csr:literal, $val:expr) => {{
        let v: usize = $val;
        unsafe {
            core::arch::asm!(concat!("csrw ", $csr, ", {0}"), in(reg) v, options(nomem, nostack));
        }
    }};
}

macro_rules! csrs {
    ($csr:literal, $val:expr) => {{
        let v: usize = $val;
        unsafe {
            core::arch::asm!(concat!("csrs ", $csr, ", {0}"), in(reg) v, options(nomem, nostack));
        }
    }};
}

macro_rules! csrc {
    ($csr:literal, $val:expr) => {{
        let v: usize = $val;
        unsafe {
            core::arch::asm!(concat!("csrc ", $csr, ", {0}"), in(reg) v, options(nomem, nostack));
        }
    }};
}

pub const SSTATUS_SIE: usize = 1 << 1;
pub const SSTATUS_SPIE: usize = 1 << 5;
pub const SSTATUS_SPP: usize = 1 << 8;
pub const SSTATUS_SUM: usize = 1 << 18;

pub const SIE_SSIE: usize = 1 << 1;
pub const SIE_STIE: usize = 1 << 5;
pub const SIE_SEIE: usize = 1 << 9;

/// scause values
pub const SCAUSE_INST_MISALIGNED: usize = 0;
pub const SCAUSE_INST_ACCESS: usize = 1;
pub const SCAUSE_ILLEGAL_INST: usize = 2;
pub const SCAUSE_BREAKPOINT: usize = 3;
pub const SCAUSE_LOAD_MISALIGNED: usize = 4;
pub const SCAUSE_LOAD_ACCESS: usize = 5;
pub const SCAUSE_STORE_MISALIGNED: usize = 6;
pub const SCAUSE_STORE_ACCESS: usize = 7;
pub const SCAUSE_ECALL_U: usize = 8;
pub const SCAUSE_ECALL_S: usize = 9;
pub const SCAUSE_INST_PAGE_FAULT: usize = 12;
pub const SCAUSE_LOAD_PAGE_FAULT: usize = 13;
pub const SCAUSE_STORE_PAGE_FAULT: usize = 15;
pub const SCAUSE_TIMER: usize = 1 | (1 << 63);
pub const SCAUSE_EXTERNAL: usize = 1 | (1 << 63) | (1 << 62);

#[inline(always)]
pub fn sstatus() -> usize {
    csrr!("sstatus")
}
#[inline(always)]
pub fn set_sstatus(v: usize) {
    csrw!("sstatus", v)
}
#[inline(always)]
pub fn sie() -> usize {
    csrr!("sie")
}
#[inline(always)]
pub fn set_sie(v: usize) {
    csrw!("sie", v)
}
#[inline(always)]
pub fn satp() -> usize {
    csrr!("satp")
}
#[inline(always)]
pub fn set_satp(v: usize) {
    csrw!("satp", v);
    sfence_vma();
}
#[inline(always)]
pub fn sepc() -> usize {
    csrr!("sepc")
}
#[inline(always)]
pub fn set_sepc(v: usize) {
    csrw!("sepc", v)
}
#[inline(always)]
pub fn scause() -> usize {
    csrr!("scause")
}
#[inline(always)]
pub fn stval() -> usize {
    csrr!("stval")
}
#[inline(always)]
pub fn stvec() -> usize {
    csrr!("stvec")
}
#[inline(always)]
pub fn set_stvec(v: usize) {
    csrw!("stvec", v)
}
#[inline(always)]
pub fn time() -> usize {
    csrr!("time")
}
#[inline(always)]
pub fn sfence_vma() {
    unsafe {
        core::arch::asm!("sfence.vma zero, zero", options(nostack, preserves_flags));
    }
}
#[inline(always)]
pub fn disable_interrupts() {
    csrc!("sstatus", SSTATUS_SIE);
}
#[inline(always)]
pub fn enable_interrupts() {
    csrs!("sstatus", SSTATUS_SIE);
}
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    sstatus() & SSTATUS_SIE != 0
}
/// Read sscratch (holds the current user trap frame while in user mode).
#[inline(always)]
pub fn sscratch() -> usize {
    csrr!("sscratch")
}
#[inline(always)]
pub fn set_sscratch(v: usize) {
    csrw!("sscratch", v)
}
