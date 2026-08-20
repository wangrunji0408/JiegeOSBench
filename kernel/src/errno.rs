//! Linux errno 定义（riscv64 通用）

pub type Ret = Result<usize, Errno>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Errno {
    Eperr = 1,
    Enoent = 2,
    Esrch = 3,
    Eintr = 4,
    Eio = 5,
    Enxio = 6,
    E2big = 7,
    Enoexec = 8,
    Ebadf = 9,
    Echild = 10,
    Eagain = 11,
    Enomem = 12,
    Eacces = 13,
    Efault = 14,
    Enotblk = 15,
    Ebusy = 16,
    Eexist = 17,
    Exdev = 18,
    Enodev = 19,
    Enotdir = 20,
    Eisdir = 21,
    Einval = 22,
    Enfile = 23,
    Emfile = 24,
    Enotty = 25,
    Etxtbsy = 26,
    Efbig = 27,
    Enospc = 28,
    Espipe = 29,
    Erofs = 30,
    Emlink = 31,
    Epipe = 32,
    Edom = 33,
    Erange = 34,
    Enomsg = 42,
    Eidrm = 43,
    Enolck = 46,
    Enosys = 38,
    Enotsock = 88,
    Edestaddrreq = 89,
    Emsgsize = 90,
    Eprototype = 91,
    Enoprotoopt = 92,
    Eprotonosupport = 93,
    Eopnotsupp = 95,
    Eafnosupport = 97,
    Eaddrinuse = 98,
    Eaddrnotavail = 99,
    Enetdown = 100,
    Enetunreach = 101,
    Econnaborted = 103,
    Econnreset = 104,
    Enobufs = 105,
    Eisconn = 106,
    Enotconn = 107,
    Eshutdown = 108,
    Etoomanyrefs = 109,
    Etimedout = 110,
    Econnrefused = 111,
    Ehostdown = 112,
    Ehostunreach = 113,
    Ealready = 114,
    Einprogress = 115,
    Edquot = 122,
    Estale = 116,
    // 信号值（用于 die）
    Sigkill = 9,
    Sigsegv = 11,
    Sigill = 4,
    Sigbus = 7,
    Sigfpe = 8,
    Sigabrt = 6,
}

impl Errno {
    pub fn code(&self) -> i64 {
        *self as i64
    }
}

pub fn ret_i64(r: Ret) -> i64 {
    match r {
        Ok(v) => v as i64,
        Err(e) => -e.code(),
    }
}
