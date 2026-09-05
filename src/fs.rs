use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    vec::Vec,
};
include!(concat!(env!("OUT_DIR"), "/rootfs.rs"));
static mut DIRS: Option<BTreeSet<String>> = None;
pub static mut VFS: Option<BTreeMap<String, Vec<u8>>> = None;
pub unsafe fn init() {
    DIRS = Some(BTreeSet::new());
    let mut m = BTreeMap::new();
    for &(p, b) in FILES {
        m.insert(p.to_string(), b.to_vec());
    }
    m.insert("/etc/passwd".into(),b"root:x:0:0:root:/:/bin/sh\nnginx:x:101:101:nginx:/:/sbin/nologin\nnobody:x:65534:65534:nobody:/:/sbin/nologin\n".to_vec());
    m.insert(
        "/etc/group".into(),
        b"root:x:0:\nnginx:x:101:\nnogroup:x:65534:\n".to_vec(),
    );
    m.insert("/proc/sys/kernel/ngroups_max".into(), b"65536\n".to_vec());
    VFS = Some(m);
}
pub fn normalize(p: &str) -> String {
    let p = match p {
        "/lib/libc.musl-riscv64.so.1" | "/usr/lib/libc.musl-riscv64.so.1" => {
            "/lib/ld-musl-riscv64.so.1"
        }
        "/usr/lib/libpcre2-8.so.0" => "/usr/lib/libpcre2-8.so.0.16.0",
        "/lib/libz.so.1" | "/usr/lib/libz.so.1" => "/usr/lib/libz.so.1.3.2",
        _ => p,
    };
    let mut s = String::new();
    for part in p.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        s.push('/');
        s.push_str(part);
    }
    if s.is_empty() {
        s.push('/');
    }
    s
}
pub fn file_data(p: &str) -> Option<Vec<u8>> {
    unsafe { VFS.as_ref().unwrap().get(&normalize(p)).cloned() }
}
pub fn exists(p: &str) -> bool {
    file_data(p).is_some() || is_dir(p)
}
pub fn is_dir(p: &str) -> bool {
    let p = normalize(p);
    let mut pref = p.clone();
    if p != "/" {
        pref.push('/');
    }
    (unsafe { DIRS.as_ref().unwrap().contains(&p) })
        || p == "/"
        || p == "/tmp"
        || p == "/run"
        || p == "/var/log/nginx"
        || unsafe { VFS.as_ref().unwrap().keys().any(|k| k.starts_with(&pref)) }
}
pub fn create(p: &str) {
    unsafe {
        VFS.as_mut().unwrap().entry(normalize(p)).or_default();
    }
}
pub fn write(p: &str, off: usize, b: &[u8]) {
    unsafe {
        let f = VFS.as_mut().unwrap().entry(normalize(p)).or_default();
        if f.len() < off + b.len() {
            f.resize(off + b.len(), 0);
        }
        f[off..off + b.len()].copy_from_slice(b);
    }
}

pub fn mkdir(p: &str) {
    unsafe {
        DIRS.as_mut().unwrap().insert(normalize(p));
    }
}
