//! Character devices under `/dev`.

use super::inode::{next_ino, Inode, InodeKind, InodeRef};
use super::path;
use super::Result;
use crate::impl_as_any;
use alloc::sync::Arc;
use spin::Mutex;

/// `/dev/null`: reads return EOF, writes are discarded.
pub struct Null {
    ino: u64,
}

impl Inode for Null {
    fn kind(&self) -> InodeKind {
        InodeKind::CharDevice
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o666
    }
    fn device(&self) -> (u32, u32) {
        (1, 3)
    }
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }
    fn truncate(&self, _len: usize) -> Result<()> {
        Ok(())
    }
    impl_as_any!();
}

/// `/dev/zero`: reads return zeros.
pub struct Zero {
    ino: u64,
}

impl Inode for Zero {
    fn kind(&self) -> InodeKind {
        InodeKind::CharDevice
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o666
    }
    fn device(&self) -> (u32, u32) {
        (1, 5)
    }
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        buf.fill(0);
        Ok(buf.len())
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }
    impl_as_any!();
}

/// `/dev/random` and `/dev/urandom`.
///
/// A xorshift PRNG seeded from the cycle counter. nginx and OpenSSL read this
/// at startup; nothing here needs cryptographic strength for our purposes, but
/// we do mix in the timer so successive boots differ.
pub struct Random {
    ino: u64,
    state: Mutex<u64>,
    minor: u32,
}

impl Random {
    fn next(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
}

impl Inode for Random {
    fn kind(&self) -> InodeKind {
        InodeKind::CharDevice
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o666
    }
    fn device(&self) -> (u32, u32) {
        (1, self.minor)
    }
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut state = self.state.lock();
        *state ^= crate::arch::cycle();
        for chunk in buf.chunks_mut(8) {
            let v = Self::next(&mut state).to_le_bytes();
            let n = chunk.len();
            chunk.copy_from_slice(&v[..n]);
        }
        Ok(buf.len())
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        // Writing seeds the pool.
        let mut state = self.state.lock();
        for &b in buf {
            *state = state.rotate_left(8) ^ b as u64;
        }
        Ok(buf.len())
    }
    impl_as_any!();
}

/// Fill a buffer with pseudo-random bytes (used for `AT_RANDOM` and
/// `getrandom`).
pub fn fill_random(buf: &mut [u8]) {
    static STATE: Mutex<u64> = Mutex::new(0x2545_F491_4F6C_DD1D);
    let mut state = STATE.lock();
    *state ^= crate::arch::cycle();
    for chunk in buf.chunks_mut(8) {
        let v = Random::next(&mut state).to_le_bytes();
        let n = chunk.len();
        chunk.copy_from_slice(&v[..n]);
    }
}

/// `/dev/console`, `/dev/tty`, and the stdio streams: the SBI console.
pub struct Tty {
    ino: u64,
}

/// Terminal ioctls.
const TCGETS: usize = 0x5401;
const TCSETS: usize = 0x5402;
const TCSETSW: usize = 0x5403;
const TCSETSF: usize = 0x5404;
const TIOCGWINSZ: usize = 0x5413;
const TIOCSWINSZ: usize = 0x5414;
const TIOCGPGRP: usize = 0x540f;
const TIOCSPGRP: usize = 0x5410;
const FIONREAD: usize = 0x541b;
const FIONBIO: usize = 0x5421;
const TIOCSCTTY: usize = 0x540e;
const TIOCNOTTY: usize = 0x5422;

/// `struct termios`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

/// `struct winsize`.
#[repr(C)]
#[derive(Clone, Copy)]
struct WinSize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

static TERMIOS: Mutex<Termios> = Mutex::new(Termios {
    // ICRNL | IXON
    c_iflag: 0o400 | 0o2000,
    // OPOST | ONLCR
    c_oflag: 0o1 | 0o4,
    // B38400 | CS8 | CREAD
    c_cflag: 0o17 | 0o60 | 0o200,
    // ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN
    c_lflag: 0o1 | 0o2 | 0o10 | 0o20 | 0o40 | 0o100000,
    c_line: 0,
    c_cc: [
        3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 18, 15, 23, 22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0,
    ],
    c_ispeed: 38400,
    c_ospeed: 38400,
});

impl Inode for Tty {
    fn kind(&self) -> InodeKind {
        InodeKind::CharDevice
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o620
    }
    fn device(&self) -> (u32, u32) {
        (5, 1)
    }

    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        // Non-blocking drain of the SBI console.
        let mut n = 0;
        while n < buf.len() {
            match crate::sbi::console_getchar() {
                Some(c) => {
                    buf[n] = if c == b'\r' { b'\n' } else { c };
                    n += 1;
                    if buf[n - 1] == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        Ok(n)
    }

    fn read(&self, _offset: usize, buf: &mut [u8], nonblock: bool) -> Result<usize> {
        loop {
            let n = self.read_at(0, buf)?;
            if n > 0 {
                return Ok(n);
            }
            if nonblock {
                crate::bail!(EAGAIN);
            }
            // No input available: yield so we don't spin the console.
            crate::task::yield_now();
            if crate::task::has_pending_signal() {
                crate::bail!(EINTR);
            }
        }
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        crate::console::write_bytes(buf);
        Ok(buf.len())
    }

    fn poll_readable(&self) -> bool {
        // We can't peek at the SBI console without consuming, so report not
        // ready; nginx never polls the tty on the request path.
        false
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        use crate::mm::uaccess;
        match cmd {
            TCGETS => {
                uaccess::write(arg, *TERMIOS.lock())?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF => {
                let t: Termios = uaccess::read(arg)?;
                *TERMIOS.lock() = t;
                Ok(0)
            }
            TIOCGWINSZ => {
                uaccess::write(
                    arg,
                    WinSize {
                        row: 24,
                        col: 80,
                        xpixel: 0,
                        ypixel: 0,
                    },
                )?;
                Ok(0)
            }
            TIOCSWINSZ | TIOCSCTTY | TIOCNOTTY | FIONBIO => Ok(0),
            TIOCGPGRP => {
                uaccess::write(arg, crate::task::current().pgid() as u32)?;
                Ok(0)
            }
            TIOCSPGRP => Ok(0),
            FIONREAD => {
                uaccess::write(arg, 0u32)?;
                Ok(0)
            }
            _ => crate::bail!(ENOTTY),
        }
    }

    impl_as_any!();
}

/// Create the standard device nodes.
pub fn init() {
    let dev = path::mkdir_p("/dev", 0o755).expect("cannot create /dev");
    let nodes: [(&str, InodeRef); 8] = [
        ("null", Arc::new(Null { ino: next_ino() })),
        ("zero", Arc::new(Zero { ino: next_ino() })),
        (
            "full",
            Arc::new(Zero { ino: next_ino() }),
        ),
        (
            "random",
            Arc::new(Random {
                ino: next_ino(),
                state: Mutex::new(0x9E37_79B9_7F4A_7C15),
                minor: 8,
            }),
        ),
        (
            "urandom",
            Arc::new(Random {
                ino: next_ino(),
                state: Mutex::new(0xBF58_476D_1CE4_E5B9),
                minor: 9,
            }),
        ),
        ("console", Arc::new(Tty { ino: next_ino() })),
        ("tty", Arc::new(Tty { ino: next_ino() })),
        ("ttyS0", Arc::new(Tty { ino: next_ino() })),
    ];
    for (name, inode) in nodes {
        let _ = dev.unlink(name);
        let _ = dev.link(name, &inode);
    }

    // Symlinks that programs expect.
    let _ = dev.symlink("stdin", "/proc/self/fd/0");
    let _ = dev.symlink("stdout", "/proc/self/fd/1");
    let _ = dev.symlink("stderr", "/proc/self/fd/2");
    let _ = path::mkdir_p("/dev/shm", 0o1777);
    let _ = path::mkdir_p("/dev/pts", 0o755);
}

/// A fresh tty inode, for the initial stdio descriptors.
pub fn new_tty() -> InodeRef {
    Arc::new(Tty { ino: next_ino() })
}
