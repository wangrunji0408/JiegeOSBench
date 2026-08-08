pub const NGINX: usize = 0;
pub const LOADER: usize = 1;
pub const CACHE: usize = 12;

pub struct EmbeddedFile {
    pub path: &'static str,
    pub data: &'static [u8],
}

// Minimal valid TZif v2 file for UTC.  glibc accepts this as /etc/localtime
// without needing the rest of the zoneinfo database.
const fn utc_tz() -> [u8; 114] {
    let mut b = [0u8; 114];
    b[0] = b'T'; b[1] = b'Z'; b[2] = b'i'; b[3] = b'f'; b[4] = b'2';
    b[36] = 0; b[37] = 0; b[38] = 0; b[39] = 1;
    b[40] = 0; b[41] = 0; b[42] = 0; b[43] = 4;
    b[50] = b'U'; b[51] = b'T'; b[52] = b'C'; b[53] = 0;
    b[54] = b'T'; b[55] = b'Z'; b[56] = b'i'; b[57] = b'f'; b[58] = b'2';
    b[90] = 0; b[91] = 0; b[92] = 0; b[93] = 1;
    b[94] = 0; b[95] = 0; b[96] = 0; b[97] = 4;
    b[104] = b'U'; b[105] = b'T'; b[106] = b'C'; b[107] = 0;
    b[108] = b'\n'; b[109] = b'U'; b[110] = b'T'; b[111] = b'C'; b[112] = b'0'; b[113] = b'\n';
    b
}

static UTC_TZ: [u8; 114] = utc_tz();

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
    EmbeddedFile { path: "/etc/localtime", data: &UTC_TZ },
    EmbeddedFile { path: "/etc/passwd", data: include_bytes!("../assets/passwd") },
    EmbeddedFile { path: "/etc/group", data: include_bytes!("../assets/group") },
    EmbeddedFile { path: "/etc/nsswitch.conf", data: include_bytes!("../assets/nsswitch.conf") },
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

pub fn exists(path: &str) -> bool {
    lookup(path).is_some()
        || matches!(path, "/" | "/dev/null" | "/dev/stderr" | "/dev/stdout")
        || path == "/var/lib" || path == "/var/lib/nginx" || path.starts_with("/var/lib/nginx/")
}
