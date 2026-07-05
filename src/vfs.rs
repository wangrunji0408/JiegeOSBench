//! 最小虚拟文件系统：以嵌入的静态文件表为后端，支持 openat/read/write/lseek/stat。
//! 动态文件系统（virtio-blk + ext2）在 Phase 7 后期接入。

use alloc::string::String;
use alloc::vec::Vec;

/// 嵌入的根文件系统文件表
pub static FILES: &[(&str, &[u8])] = &[
    ("/etc/hostname", b"ijiege-os\n"),
    ("/etc/passwd", b"root:x:0:0:root:/root:/bin/sh\n"),
    ("/etc/group", b"root:x:0:\n"),
    ("/index.html", b"<!DOCTYPE html>\n<html><head><title>ijiege-os</title></head>\n<body><h1>Hello from nginx on a from-scratch RISC-V kernel!</h1></body></html>\n"),
    ("/www/index.html", b"<!DOCTYPE html>\n<html><head><title>nginx @ ijiege-os</title></head>\n<body><h1>Hello from nginx on a from-scratch RISC-V kernel!</h1><p>Served by nginx official binary.</p></body></html>\n"),
    ("/nginx.conf", b"daemon off;\nmaster_process off;\nworker_processes 1;\nevents { worker_connections 16; }\nhttp {\n    access_log off;\n    error_log /dev/null;\n    server { listen 80; root /www; index index.html; }\n}\n"),
    ("/usr/local/nginx/conf/nginx.conf", b"daemon off;\nmaster_process off;\nworker_processes 1;\nevents { worker_connections 16; }\nhttp {\n    access_log off;\n    error_log /dev/null;\n    server { listen 80; root /www; index index.html; }\n}\n"),
    ("/usr/local/nginx/html/index.html", b"<!DOCTYPE html>\n<html><body><h1>nginx @ ijiege-os</h1></body></html>\n"),
    ("/dev/null", b""),
];

/// 文件描述符表项
#[derive(Clone)]
pub struct File {
    pub path: String,
    pub data: &'static [u8],
    pub offset: usize,
    pub writable: bool, // 写文件（暂只支持内存读写）
}

pub struct FdTable {
    pub fds: Vec<Option<File>>,
}

impl FdTable {
    pub fn new() -> Self {
        let mut fds: Vec<Option<File>> = Vec::new();
        fds.resize_with(16, || None);
        // 预置 stdin/stdout/stderr
        fds[0] = Some(File { path: String::from("/dev/stdin"), data: &[], offset: 0, writable: true });
        fds[1] = Some(File { path: String::from("/dev/stdout"), data: &[], offset: 0, writable: true });
        fds[2] = Some(File { path: String::from("/dev/stderr"), data: &[], offset: 0, writable: true });
        Self { fds }
    }

    pub fn open(&mut self, path: &str, _flags: usize) -> Option<usize> {
        // /dev/stdin / stdout / stderr
        if path == "/dev/stdin" || path == "/dev/stdout" || path == "/dev/stderr" {
            return Some(self.alloc_fd(File {
                path: String::from(path),
                data: &[],
                offset: 0,
                writable: true,
            }));
        }
        for (p, data) in FILES.iter() {
            if *p == path {
                return Some(self.alloc_fd(File {
                    path: String::from(path),
                    data,
                    offset: 0,
                    writable: false,
                }));
            }
        }
        None
    }

    fn alloc_fd(&mut self, f: File) -> usize {
        for i in 3..self.fds.len() {
            if self.fds[i].is_none() {
                self.fds[i] = Some(f);
                return i;
            }
        }
        let i = self.fds.len();
        self.fds.push(Some(f));
        i
    }

    pub fn close(&mut self, fd: usize) -> bool {
        if fd < self.fds.len() {
            self.fds[fd] = None;
            true
        } else {
            false
        }
    }

    pub fn read(&mut self, fd: usize, buf: &mut [u8]) -> Option<usize> {
        let f = self.fds.get_mut(fd)?.as_mut()?;
        if f.path == "/dev/stdin" {
            let mut n = 0;
            while n < buf.len() {
                match crate::uart::getc() {
                    Some(c) => {
                        buf[n] = c;
                        n += 1;
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                    }
                    None => break,
                }
            }
            return Some(n);
        }
        let avail = f.data.len().saturating_sub(f.offset);
        let n = avail.min(buf.len());
        buf[..n].copy_from_slice(&f.data[f.offset..f.offset + n]);
        f.offset += n;
        Some(n)
    }

    pub fn write(&mut self, fd: usize, buf: &[u8]) -> Option<usize> {
        let f = self.fds.get(fd)?.as_ref()?;
        if f.path == "/dev/stdout" || f.path == "/dev/stderr" || fd <= 2 {
            for &b in buf {
                crate::uart::putc(b);
            }
            return Some(buf.len());
        }
        // 其他文件暂不支持写
        Some(buf.len())
    }

    pub fn lseek(&mut self, fd: usize, offset: isize, whence: usize) -> Option<usize> {
        let f = self.fds.get_mut(fd)?.as_mut()?;
        let new = match whence {
            0 => offset as usize,            // SEEK_SET
            1 => (f.offset as isize + offset) as usize, // SEEK_CUR
            2 => (f.data.len() as isize + offset) as usize, // SEEK_END
            _ => return None,
        };
        f.offset = new.min(f.data.len());
        Some(f.offset)
    }

    pub fn pread(&self, fd: usize, buf: &mut [u8], offset: usize) -> Option<usize> {
        let f = self.fds.get(fd)?.as_ref()?;
        if f.path == "/dev/stdin" {
            return Some(0);
        }
        if offset >= f.data.len() {
            return Some(0);
        }
        let n = (f.data.len() - offset).min(buf.len());
        buf[..n].copy_from_slice(&f.data[offset..offset + n]);
        Some(n)
    }

    pub fn stat(&self, fd: usize, statbuf: usize) -> bool {
        // 复用 fstat 布局
        if statbuf == 0 {
            return false;
        }
        for i in 0..144usize {
            unsafe { core::ptr::write_volatile((statbuf + i) as *mut u8, 0); }
        }
        let f = match self.fds.get(fd).and_then(|x| x.as_ref()) {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            core::ptr::write_volatile((statbuf + 16) as *mut u64, 1); // nlink
            core::ptr::write_volatile((statbuf + 24) as *mut u32, 0x81a4); // mode
            core::ptr::write_volatile((statbuf + 48) as *mut i64, f.data.len() as i64);
            core::ptr::write_volatile((statbuf + 56) as *mut u64, 4096);
        }
        true
    }
}

/// 由路径查找 stat（openat 失败时用 stat 路径）
pub fn stat_path(path: &str, statbuf: usize) -> bool {
    if statbuf == 0 {
        return false;
    }
    for i in 0..144usize {
        unsafe { core::ptr::write_volatile((statbuf + i) as *mut u8, 0); }
    }
    let len = if path == "/dev/stdin" || path == "/dev/stdout" || path == "/dev/stderr" {
        0
    } else {
        FILES.iter().find(|(p, _)| *p == path).map(|(_, d)| d.len()).unwrap_or(0)
    };
    unsafe {
        core::ptr::write_volatile((statbuf + 16) as *mut u64, 1);
        core::ptr::write_volatile((statbuf + 24) as *mut u32, 0x81a4);
        core::ptr::write_volatile((statbuf + 48) as *mut i64, len as i64);
        core::ptr::write_volatile((statbuf + 56) as *mut u64, 4096);
    }
    true
}
