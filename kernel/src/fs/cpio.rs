//! cpio "newc" archive parser (the Linux initramfs format).

pub const NEWC_MAGIC: &[u8; 6] = b"070701";

pub struct CpioEntry<'a> {
    pub name: &'a str,
    pub mode: u32,
    pub ino: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub data: &'a [u8],
}

fn hex8(b: &[u8]) -> u32 {
    let mut v = 0u32;
    for &c in b.iter().take(8) {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        };
        v = (v << 4) | d as u32;
    }
    v
}

/// Iterate over the archive, calling `f` for every entry.
pub fn walk<F: FnMut(CpioEntry<'static>)>(buf: &'static [u8], mut f: F) {
    let mut off = 0usize;
    while off + 110 <= buf.len() {
        if &buf[off..off + 6] != NEWC_MAGIC {
            break;
        }
        let h = &buf[off..off + 110];
        let ino = hex8(&h[6..14]);
        let mode = hex8(&h[14..22]);
        let uid = hex8(&h[22..30]);
        let gid = hex8(&h[30..38]);
        let nlink = hex8(&h[38..46]);
        let filesize = hex8(&h[54..62]) as usize;
        let rdevmaj = hex8(&h[62..70]);
        let rdevmin = hex8(&h[70..78]);
        let namesize = hex8(&h[94..102]) as usize;
        let name_off = off + 110;
        if name_off + namesize > buf.len() {
            break;
        }
        let name_bytes = &buf[name_off..name_off + namesize];
        let name = core::str::from_utf8(name_bytes)
            .unwrap_or("")
            .trim_end_matches('\0');
        if name == "TRAILER!!!" {
            break;
        }
        let data_off = (name_off + namesize + 3) & !3;
        if data_off + filesize > buf.len() {
            break;
        }
        let data = &buf[data_off..data_off + filesize];
        f(CpioEntry {
            name,
            mode,
            ino,
            nlink,
            uid,
            gid,
            rdev: (rdevmaj << 8) | (rdevmin & 0xff),
            data,
        });
        off = (data_off + filesize + 3) & !3;
    }
}
