pub const NGINX: usize = 0;
pub const LOADER: usize = 1;

pub struct EmbeddedFile {
    pub path: &'static str,
    pub data: &'static [u8],
}

static FILES: &[EmbeddedFile] = &[
    EmbeddedFile { path: "/usr/sbin/nginx", data: include_bytes!("../assets/deb-extracted/usr/sbin/nginx") },
    EmbeddedFile { path: "/lib/ld-linux-riscv64-lp64d.so.1", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libc.so.6", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libc.so.6") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libcrypt.so.1.1.0", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libcrypt.so.1.1.0") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libpcre2-8.so.0.14.0", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libpcre2-8.so.0.14.0") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libssl.so.3", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libssl.so.3") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libcrypto.so.3", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libcrypto.so.3") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libz.so.1.3.1", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libz.so.1.3.1") },
    EmbeddedFile { path: "/lib/riscv64-linux-gnu/libzstd.so.1.5.7", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/libzstd.so.1.5.7") },
    EmbeddedFile { path: "/usr/lib/riscv64-linux-gnu/ossl-modules/legacy.so", data: include_bytes!("../assets/rootfs/usr/lib/riscv64-linux-gnu/ossl-modules/legacy.so") },
    EmbeddedFile { path: "/etc/nginx/nginx.conf", data: include_bytes!("../assets/etc-nginx.conf") },
    EmbeddedFile { path: "/var/www/index.html", data: include_bytes!("../assets/index.html") },
    EmbeddedFile { path: "/etc/ld.so.cache", data: include_bytes!("../assets/ubuntu-ld.so.cache") },
    EmbeddedFile { path: "/usr/lib/ssl/openssl.cnf", data: include_bytes!("../assets/openssl.cnf") },
    EmbeddedFile { path: "/etc/ssl/openssl.cnf", data: include_bytes!("../assets/openssl.cnf") },
    EmbeddedFile { path: "/sys/devices/system/cpu/online", data: include_bytes!("../assets/cpu-online") },
    EmbeddedFile { path: "/proc/stat", data: include_bytes!("../assets/proc-stat") },
];

pub fn data(index: usize) -> Option<&'static [u8]> { FILES.get(index).map(|f| f.data) }

fn basename(path: &str) -> &str { path.rsplit('/').next().unwrap_or(path) }

pub fn lookup(path: &str) -> Option<usize> {
    let trimmed = if path.is_empty() { "/" } else { path };
    for (i, file) in FILES.iter().enumerate() {
        if file.path == trimmed { return Some(i); }
    }
    match basename(trimmed) {
        "ld-linux-riscv64-lp64d.so.1" => Some(LOADER),
        "libc.so.6" => Some(2),
        "libcrypt.so.1" | "libcrypt.so.1.1.0" => Some(3),
        "libpcre2-8.so.0" | "libpcre2-8.so.0.14.0" => Some(4),
        "libssl.so.3" => Some(5),
        "libcrypto.so.3" => Some(6),
        "libz.so.1" | "libz.so.1.3.1" => Some(7),
        "libzstd.so.1" | "libzstd.so.1.5.7" => Some(8),
        "legacy.so" => Some(9),
        _ => None,
    }
}

pub fn exists(path: &str) -> bool { lookup(path).is_some() || matches!(path, "/" | "/dev/null" | "/dev/stderr" | "/dev/stdout") }
