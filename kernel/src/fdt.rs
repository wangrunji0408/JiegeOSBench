//! Minimal flattened device tree parser: enough to find RAM and the initrd.

pub struct Fdt {
    ptr: usize,
}

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

impl Fdt {
    pub fn new(ptr: usize) -> Option<Self> {
        let f = Self { ptr };
        if f.u32be(0) == FDT_MAGIC {
            Some(f)
        } else {
            None
        }
    }

    fn u32be(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_unaligned((self.ptr + off) as *const u32) }.swap_bytes()
    }

    fn off_struct(&self) -> usize {
        self.u32be(8) as usize
    }
    fn off_strings(&self) -> usize {
        self.u32be(12) as usize
    }
    fn total_size(&self) -> usize {
        self.u32be(4) as usize
    }

    fn string_at(&self, off: usize) -> &'static str {
        let mut p = self.ptr + self.off_strings() + off;
        let mut len = 0;
        unsafe {
            while *(p as *const u8) != 0 {
                p += 1;
                len += 1;
            }
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                (self.ptr + self.off_strings() + off) as *const u8,
                len,
            ))
        }
    }

    fn prop_u64(&self, data: usize, len: usize) -> u64 {
        unsafe {
            if len >= 8 {
                core::ptr::read_unaligned(data as *const u64).swap_bytes()
            } else if len >= 4 {
                core::ptr::read_unaligned(data as *const u32).swap_bytes() as u64
            } else {
                0
            }
        }
    }

    /// Walk the tree, calling `f(node_name, prop_name, data_ptr, len)` for every property.
    pub fn walk<F: FnMut(&str, &str, usize, usize)>(&self, mut f: F) {
        let mut p = self.ptr + self.off_struct();
        let end = self.ptr + self.total_size();
        let mut stack: heapless_names::NameStack = heapless_names::NameStack::new();
        while p < end {
            let tok = unsafe { core::ptr::read_unaligned(p as *const u32) }.swap_bytes();
            p += 4;
            match tok {
                FDT_BEGIN_NODE => {
                    let mut len = 0;
                    unsafe {
                        while *(p as *const u8).add(len) != 0 {
                            len += 1;
                        }
                    }
                    let name = unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            p as *const u8,
                            len,
                        ))
                    };
                    stack.push(name);
                    p += (len + 1 + 3) & !3;
                }
                FDT_END_NODE => {
                    stack.pop();
                }
                FDT_PROP => {
                    let len = unsafe { core::ptr::read_unaligned(p as *const u32) }.swap_bytes() as usize;
                    let nameoff =
                        unsafe { core::ptr::read_unaligned((p + 4) as *const u32) }.swap_bytes() as usize;
                    p += 8;
                    let pname = self.string_at(nameoff);
                    let node = stack.current();
                    f(node, pname, p, len);
                    p += (len + 3) & !3;
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => break,
            }
        }
    }

    pub fn memory(&self) -> Option<(usize, usize)> {
        let mut result = None;
        self.walk(|node, prop, data, len| {
            if prop == "device_type" && result.is_none() {
                // not used
            }
            if prop == "reg" && node.starts_with("memory") && len >= 16 && result.is_none() {
                let base = self.prop_u64(data, 8);
                let size = self.prop_u64(data + 8, 8);
                result = Some((base as usize, size as usize));
            }
        });
        result
    }

    pub fn initrd(&self) -> Option<(usize, usize)> {
        let mut start = None;
        let mut end = None;
        self.walk(|node, prop, data, len| {
            if node.starts_with("chosen") {
                if prop == "linux,initrd-start" {
                    start = Some(self.prop_u64(data, len) as usize);
                } else if prop == "linux,initrd-end" {
                    end = Some(self.prop_u64(data, len) as usize);
                }
            }
        });
        match (start, end) {
            (Some(s), Some(e)) if e > s => Some((s, e)),
            _ => None,
        }
    }

    pub fn bootargs(&self) -> Option<&'static str> {
        let mut res = None;
        self.walk(|node, prop, data, len| {
            if node.starts_with("chosen") && prop == "bootargs" && res.is_none() {
                let s = unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(data as *const u8, len))
                };
                res = Some(s.trim_end_matches('\0'));
            }
        });
        res
    }
}

mod heapless_names {
    pub struct NameStack {
        buf: [u8; 256],
        len: usize,
    }
    impl NameStack {
        pub fn new() -> Self {
            Self {
                buf: [0; 256],
                len: 0,
            }
        }
        pub fn push(&mut self, name: &str) {
            self.len = 0;
            let n = name.len().min(self.buf.len());
            self.buf[..n].copy_from_slice(&name.as_bytes()[..n]);
            self.len = n;
        }
        pub fn pop(&mut self) {
            self.len = 0;
        }
        pub fn current(&self) -> &str {
            unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
        }
    }
}
