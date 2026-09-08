//! Character devices: /dev/null, zero, full, random, urandom, console.

use crate::errno::*;
use crate::sync::SpinLock;

pub const DEV_NULL: u32 = (1 << 8) | 3;
pub const DEV_ZERO: u32 = (1 << 8) | 5;
pub const DEV_FULL: u32 = (1 << 8) | 7;
pub const DEV_RANDOM: u32 = (1 << 8) | 8;
pub const DEV_URANDOM: u32 = (1 << 8) | 9;
pub const DEV_CONSOLE: u32 = (5 << 8) | 1;

struct Rng(u64);
static RNG: SpinLock<Rng> = SpinLock::new(Rng(0x2545_F491_4F6C_DD1D));

fn next_rand() -> u64 {
    let mut r = RNG.lock();
    if r.0 == 0 {
        r.0 = crate::time::rdtime() ^ 0x9E37_79B9_7F4A_7C15;
    }
    let mut x = r.0;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    r.0 = x;
    x
}

pub fn read(rdev: u32, buf: &mut [u8]) -> Result<usize, Errno> {
    match rdev {
        DEV_NULL => Ok(0),
        DEV_ZERO | DEV_FULL => {
            buf.fill(0);
            Ok(buf.len())
        }
        DEV_RANDOM | DEV_URANDOM => {
            let mut off = 0;
            while off < buf.len() {
                let v = next_rand().to_le_bytes();
                let n = core::cmp::min(8, buf.len() - off);
                buf[off..off + n].copy_from_slice(&v[..n]);
                off += n;
            }
            Ok(buf.len())
        }
        DEV_CONSOLE => {
            // poll the UART input buffer; return 0 if nothing is available
            let mut n = 0;
            while n < buf.len() {
                match crate::console::try_getchar() {
                    Some(b) => {
                        buf[n] = b;
                        n += 1;
                    }
                    None => break,
                }
            }
            Ok(n)
        }
        _ => Err(ENODEV),
    }
}

pub fn write(rdev: u32, buf: &[u8]) -> Result<usize, Errno> {
    match rdev {
        DEV_NULL | DEV_ZERO | DEV_FULL => Ok(buf.len()),
        DEV_RANDOM | DEV_URANDOM => Err(EINVAL),
        DEV_CONSOLE => {
            for &b in buf {
                crate::console::put_byte_raw(b);
            }
            Ok(buf.len())
        }
        _ => Err(ENODEV),
    }
}

pub fn readable(rdev: u32) -> bool {
    match rdev {
        DEV_NULL | DEV_ZERO | DEV_RANDOM | DEV_URANDOM => true,
        DEV_CONSOLE => crate::console::has_input(),
        _ => true,
    }
}

pub fn writable(_rdev: u32) -> bool {
    true
}
