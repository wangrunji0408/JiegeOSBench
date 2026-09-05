//! Character devices: /dev/null, /dev/zero, /dev/urandom, /dev/console.
use alloc::sync::Arc;
use core::any::Any;
use core::sync::atomic::{AtomicU64, Ordering};

use super::file::{File, FileOps};
use super::vfs::Dentry;
use crate::abi::*;
use crate::console;
use crate::mm::uaccess::{read_val, write_val};
use crate::sync::SpinLock;
use crate::task::wait::{block_on, WaitQueue};

pub const MAJOR_MEM: u32 = 1;
pub const MINOR_NULL: u32 = 3;
pub const MINOR_ZERO: u32 = 5;
pub const MINOR_RANDOM: u32 = 8;
pub const MINOR_URANDOM: u32 = 9;
pub const MAJOR_TTY: u32 = 5;
pub const MINOR_CONSOLE: u32 = 1;

pub struct NullDev {
    pub dentry: Arc<Dentry>,
    pub zero: bool,
}

impl FileOps for NullDev {
    fn read_at(&self, _off: u64, buf: &mut [u8], _file: &File) -> SysResult {
        if self.zero {
            buf.fill(0);
            Ok(buf.len())
        } else {
            Ok(0)
        }
    }
    fn write_at(&self, _off: u64, buf: &[u8], _file: &File) -> SysResult {
        Ok(buf.len())
    }
    fn stat(&self) -> Result<Stat, i32> {
        Ok(self.dentry.stat())
    }
    fn dentry(&self) -> Option<Arc<Dentry>> {
        Some(self.dentry.clone())
    }
    fn seekable(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

static RNG_STATE: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);

pub fn random_u64() -> u64 {
    // xorshift64* mixed with the timer
    let mut x = RNG_STATE.load(Ordering::Relaxed) ^ crate::trap::csr::read_time();
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RNG_STATE.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545F4914F6CDD1D)
}

pub fn fill_random(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let r = random_u64().to_le_bytes();
        let n = (buf.len() - i).min(8);
        buf[i..i + n].copy_from_slice(&r[..n]);
        i += n;
    }
}

pub struct RandomDev {
    pub dentry: Arc<Dentry>,
}

impl FileOps for RandomDev {
    fn read_at(&self, _off: u64, buf: &mut [u8], _file: &File) -> SysResult {
        fill_random(buf);
        Ok(buf.len())
    }
    fn write_at(&self, _off: u64, buf: &[u8], _file: &File) -> SysResult {
        Ok(buf.len())
    }
    fn stat(&self) -> Result<Stat, i32> {
        Ok(self.dentry.stat())
    }
    fn dentry(&self) -> Option<Arc<Dentry>> {
        Some(self.dentry.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ConsoleDev {
    pub dentry: Option<Arc<Dentry>>,
    termios: SpinLock<Termios>,
    pub fg_pgrp: SpinLock<i32>,
}

impl ConsoleDev {
    pub fn new(dentry: Option<Arc<Dentry>>) -> Self {
        let mut cc = [0u8; 19];
        cc[0] = 3; // VINTR ^C
        cc[1] = 28; // VQUIT
        cc[2] = 127; // VERASE
        cc[3] = 21; // VKILL
        cc[4] = 4; // VEOF
        cc[6] = 1; // VMIN
        cc[8] = 17; // VSTART
        cc[9] = 19; // VSTOP
        cc[10] = 26; // VSUSP
        cc[14] = 22; // VLNEXT
        cc[15] = 23; // VWERASE
        ConsoleDev {
            dentry,
            termios: SpinLock::new(Termios {
                c_iflag: 0o2400 | 0o400, // ICRNL | IXON
                c_oflag: 0o5,            // OPOST | ONLCR
                c_cflag: 0o277,          // B38400 | CS8 | CREAD
                c_lflag: 0o105073,       // ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN | ECHOCTL | ECHOKE
                c_line: 0,
                c_cc: cc,
                c_ispeed: 38400,
                c_ospeed: 38400,
            }),
            fg_pgrp: SpinLock::new(1),
        }
    }
}

impl FileOps for ConsoleDev {
    fn read_at(&self, _off: u64, buf: &mut [u8], file: &File) -> SysResult {
        block_on(&[&console::INPUT_WQ], file.nonblock(), || {
            let n = console::read_input(buf);
            if n == 0 {
                Err(EAGAIN)
            } else {
                Ok(n)
            }
        })
    }

    fn write_at(&self, _off: u64, buf: &[u8], _file: &File) -> SysResult {
        for &b in buf {
            console::putchar(b);
        }
        Ok(buf.len())
    }

    fn poll(&self) -> u32 {
        let mut ev = POLLOUT;
        if console::input_available() {
            ev |= POLLIN;
        }
        ev
    }

    fn wait_queue(&self) -> Option<&WaitQueue> {
        Some(&console::INPUT_WQ)
    }

    fn ioctl(&self, cmd: u32, arg: usize) -> SysResult {
        match cmd {
            TCGETS => {
                write_val(arg, *self.termios.lock())?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF => {
                let t: Termios = read_val(arg)?;
                console::set_echo(t.c_lflag & 0o10 != 0);
                *self.termios.lock() = t;
                Ok(0)
            }
            TIOCGWINSZ => {
                write_val(arg, Winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 })?;
                Ok(0)
            }
            TIOCSWINSZ => Ok(0),
            TIOCGPGRP => {
                write_val(arg, *self.fg_pgrp.lock())?;
                Ok(0)
            }
            TIOCSPGRP => {
                let pg: i32 = read_val(arg)?;
                *self.fg_pgrp.lock() = pg;
                Ok(0)
            }
            TIOCSCTTY => Ok(0),
            TIOCGSID => {
                write_val(arg, 1i32)?;
                Ok(0)
            }
            FIONREAD => {
                write_val(arg, if console::input_available() { 1i32 } else { 0 })?;
                Ok(0)
            }
            _ => Err(ENOTTY),
        }
    }

    fn stat(&self) -> Result<Stat, i32> {
        match &self.dentry {
            Some(d) => Ok(d.stat()),
            None => Ok(Stat { st_mode: S_IFCHR | 0o620, st_nlink: 1, st_rdev: ((MAJOR_TTY as u64) << 8) | MINOR_CONSOLE as u64, st_blksize: 1024, ..Stat::default() }),
        }
    }

    fn dentry(&self) -> Option<Arc<Dentry>> {
        self.dentry.clone()
    }

    fn is_tty(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub static CONSOLE: crate::sync::Global<Arc<ConsoleDev>> = crate::sync::Global::new();

pub fn init() {
    CONSOLE.init(Arc::new(ConsoleDev::new(None)));
}

/// Build FileOps for a character device node.
pub fn open_chardev(dentry: &Arc<Dentry>, major: u32, minor: u32) -> Result<Arc<dyn FileOps>, i32> {
    match (major, minor) {
        (MAJOR_MEM, MINOR_NULL) => Ok(Arc::new(NullDev { dentry: dentry.clone(), zero: false })),
        (MAJOR_MEM, MINOR_ZERO) => Ok(Arc::new(NullDev { dentry: dentry.clone(), zero: true })),
        (MAJOR_MEM, MINOR_RANDOM) | (MAJOR_MEM, MINOR_URANDOM) => Ok(Arc::new(RandomDev { dentry: dentry.clone() })),
        (MAJOR_TTY, _) => Ok(Arc::new(ConsoleDev::new(Some(dentry.clone())))),
        _ => Err(ENXIO),
    }
}
