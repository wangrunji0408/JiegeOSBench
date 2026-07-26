//! A minimal `/proc` and `/sys`.
//!
//! Contents are generated on read. nginx consults `/proc/sys/kernel/...` and
//! `/proc/cpuinfo`; musl reads `/proc/self/...` in a few paths.

use super::inode::{next_ino, DirEntry, Inode, InodeKind, InodeRef};
use super::{path, Result};
use crate::impl_as_any;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// A file whose contents are produced by a closure at read time.
pub struct ProcFile {
    ino: u64,
    generate: fn() -> String,
}

impl ProcFile {
    pub fn new(generate: fn() -> String) -> Arc<Self> {
        Arc::new(Self {
            ino: next_ino(),
            generate,
        })
    }
}

impl Inode for ProcFile {
    fn kind(&self) -> InodeKind {
        InodeKind::File
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o444
    }
    fn size(&self) -> usize {
        (self.generate)().len()
    }
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let content = (self.generate)();
        let data = content.as_bytes();
        if offset >= data.len() {
            return Ok(0);
        }
        let n = buf.len().min(data.len() - offset);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        Ok(n)
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        // Accept and ignore writes to sysctl-style files.
        Ok(buf.len())
    }
    impl_as_any!();
}

/// `/proc/self`: a directory whose contents depend on the calling process.
pub struct ProcSelfDir {
    ino: u64,
}

impl Inode for ProcSelfDir {
    fn kind(&self) -> InodeKind {
        InodeKind::Dir
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o555
    }
    fn size(&self) -> usize {
        4096
    }

    fn lookup(&self, name: &str) -> Result<InodeRef> {
        match name {
            "exe" => {
                let exe = crate::task::current().exe_path();
                Ok(super::ramfs::RamSymlink::new(&exe))
            }
            "cwd" => {
                let cwd = crate::task::current_cwd();
                let p = path::abs_path(&cwd).unwrap_or_else(|| "/".to_string());
                Ok(super::ramfs::RamSymlink::new(&p))
            }
            "root" => Ok(super::ramfs::RamSymlink::new("/")),
            "fd" => Ok(Arc::new(ProcFdDir { ino: next_ino() })),
            "maps" => Ok(ProcFile::new(gen_maps)),
            "stat" => Ok(ProcFile::new(gen_self_stat)),
            "status" => Ok(ProcFile::new(gen_self_status)),
            "cmdline" => Ok(ProcFile::new(gen_cmdline)),
            "environ" => Ok(ProcFile::new(|| String::new())),
            "limits" => Ok(ProcFile::new(gen_limits)),
            "oom_score_adj" | "oom_adj" => Ok(ProcFile::new(|| "0\n".to_string())),
            "auxv" => Ok(ProcFile::new(|| String::new())),
            "mounts" => Ok(ProcFile::new(gen_mounts)),
            "." => Ok(Arc::new(ProcSelfDir { ino: self.ino })),
            ".." => Ok(path::resolve_from(super::root(), "/proc", true)?),
            _ => crate::bail!(ENOENT),
        }
    }

    fn readdir(&self) -> Result<Vec<DirEntry>> {
        let names = [
            (".", InodeKind::Dir),
            ("..", InodeKind::Dir),
            ("exe", InodeKind::Symlink),
            ("cwd", InodeKind::Symlink),
            ("fd", InodeKind::Dir),
            ("maps", InodeKind::File),
            ("stat", InodeKind::File),
            ("status", InodeKind::File),
            ("cmdline", InodeKind::File),
        ];
        Ok(names
            .iter()
            .map(|(n, k)| DirEntry {
                name: n.to_string(),
                kind: *k,
                ino: next_ino(),
            })
            .collect())
    }

    impl_as_any!();
}

/// `/proc/self/fd`: nginx's `ngx_close_channel` path and some libraries walk it
/// to close inherited descriptors.
pub struct ProcFdDir {
    ino: u64,
}

impl Inode for ProcFdDir {
    fn kind(&self) -> InodeKind {
        InodeKind::Dir
    }
    fn ino(&self) -> u64 {
        self.ino
    }
    fn mode(&self) -> u32 {
        0o500
    }
    fn size(&self) -> usize {
        4096
    }

    fn lookup(&self, name: &str) -> Result<InodeRef> {
        if name == "." || name == ".." {
            return Ok(Arc::new(ProcFdDir { ino: self.ino }));
        }
        let fd: i32 = name.parse().map_err(|_| crate::err!(ENOENT))?;
        let file = crate::task::current()
            .files
            .lock()
            .get(fd)
            .ok_or(crate::err!(ENOENT))?;
        Ok(file.inode.clone())
    }

    fn readdir(&self) -> Result<Vec<DirEntry>> {
        let mut out = alloc::vec![
            DirEntry { name: ".".to_string(), kind: InodeKind::Dir, ino: self.ino },
            DirEntry { name: "..".to_string(), kind: InodeKind::Dir, ino: self.ino },
        ];
        let files: Vec<(i32, Arc<super::File>)> =
            crate::task::current().files.lock().iter().collect();
        for (fd, file) in files {
            out.push(DirEntry {
                name: alloc::format!("{}", fd),
                kind: InodeKind::Symlink,
                ino: file.inode.ino(),
            });
        }
        Ok(out)
    }

    impl_as_any!();
}

fn gen_cmdline() -> String {
    let task = crate::task::current();
    let mut s = String::new();
    for arg in task.cmdline().iter() {
        s.push_str(arg);
        s.push('\0');
    }
    s
}

fn gen_maps() -> String {
    let task = crate::task::current();
    let aspace = task.aspace.lock();
    let mut s = String::new();
    for (_, vma) in aspace.areas.iter() {
        use core::fmt::Write;
        let _ = writeln!(
            s,
            "{:012x}-{:012x} {}{}{}{} 00000000 00:00 0 {}",
            vma.start,
            vma.end,
            if vma.prot.contains(crate::mm::Prot::READ) { "r" } else { "-" },
            if vma.prot.contains(crate::mm::Prot::WRITE) { "w" } else { "-" },
            if vma.prot.contains(crate::mm::Prot::EXEC) { "x" } else { "-" },
            if vma.shared { "s" } else { "p" },
            vma.name,
        );
    }
    s
}

fn gen_self_stat() -> String {
    let task = crate::task::current();
    // Only the first few fields matter to anything that reads this.
    alloc::format!(
        "{} ({}) R {} {} {} 0 -1 0 0 0 0 0 0 0 0 20 0 {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        task.pid(),
        task.name(),
        task.ppid(),
        task.pgid(),
        task.pgid(),
        1,
    )
}

fn gen_self_status() -> String {
    let task = crate::task::current();
    let (used, _) = crate::mm::frame::stats();
    alloc::format!(
        "Name:\t{}\nState:\tR (running)\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\nVmRSS:\t{} kB\nThreads:\t1\n",
        task.name(),
        task.pid(),
        task.tid,
        task.ppid(),
        used * 4,
    )
}

fn gen_limits() -> String {
    "Max open files            1024                 65536                files\n".to_string()
}

fn gen_mounts() -> String {
    "rootfs / rootfs rw 0 0\nproc /proc proc rw,nosuid,nodev,noexec 0 0\ntmpfs /tmp tmpfs rw 0 0\n"
        .to_string()
}

fn gen_cpuinfo() -> String {
    "processor\t: 0\nhart\t\t: 0\nisa\t\t: rv64imafdc\nmmu\t\t: sv39\nuarch\t\t: jiege\n\n".to_string()
}

fn gen_meminfo() -> String {
    let (used, total) = crate::mm::frame::stats();
    alloc::format!(
        "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\nBuffers:               0 kB\nCached:                0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n",
        total * 4,
        (total - used) * 4,
        (total - used) * 4,
    )
}

fn gen_stat() -> String {
    alloc::format!(
        "cpu  0 0 0 {} 0 0 0 0 0 0\ncpu0 0 0 0 {} 0 0 0 0 0 0\nintr 0\nctxt {}\nbtime 0\nprocesses {}\nprocs_running 1\nprocs_blocked 0\n",
        crate::time::ticks(),
        crate::time::ticks(),
        crate::task::context_switches(),
        crate::task::processes_created(),
    )
}

fn gen_uptime() -> String {
    let (s, ns) = crate::time::monotonic();
    alloc::format!("{}.{:02} {}.{:02}\n", s, ns / 10_000_000, s, ns / 10_000_000)
}

fn gen_loadavg() -> String {
    alloc::format!("0.00 0.00 0.00 1/{} {}\n", 1, crate::task::processes_created())
}

/// Populate `/proc` and `/sys`.
pub fn init() {
    let proc = path::mkdir_p("/proc", 0o555).expect("cannot create /proc");
    let _ = proc.link("self", &(Arc::new(ProcSelfDir { ino: next_ino() }) as InodeRef));
    let _ = proc.link("thread-self", &(Arc::new(ProcSelfDir { ino: next_ino() }) as InodeRef));

    let files: [(&str, fn() -> String); 8] = [
        ("cpuinfo", gen_cpuinfo),
        ("meminfo", gen_meminfo),
        ("stat", gen_stat),
        ("uptime", gen_uptime),
        ("loadavg", gen_loadavg),
        ("mounts", gen_mounts),
        ("filesystems", || "nodev\trootfs\nnodev\tproc\nnodev\ttmpfs\n".to_string()),
        ("version", || {
            "Linux version 6.6.0-jiege (jiege@localhost) #1 SMP riscv64\n".to_string()
        }),
    ];
    for (name, gen) in files {
        let _ = proc.link(name, &(ProcFile::new(gen) as InodeRef));
    }

    // sysctl-style knobs. nginx reads `somaxconn` to size its listen backlog.
    let sysctls: [(&str, &str); 10] = [
        ("/proc/sys/kernel/ostype", "Linux\n"),
        ("/proc/sys/kernel/osrelease", "6.6.0-jiege\n"),
        ("/proc/sys/kernel/hostname", "jiege\n"),
        ("/proc/sys/kernel/pid_max", "32768\n"),
        ("/proc/sys/kernel/random/boot_id", "00000000-0000-4000-8000-000000000000\n"),
        ("/proc/sys/kernel/random/uuid", "00000000-0000-4000-8000-000000000001\n"),
        ("/proc/sys/net/core/somaxconn", "4096\n"),
        ("/proc/sys/net/ipv4/ip_local_port_range", "32768\t60999\n"),
        ("/proc/sys/net/ipv4/tcp_max_syn_backlog", "1024\n"),
        ("/proc/sys/vm/overcommit_memory", "0\n"),
    ];
    for (p, content) in sysctls {
        // These are writable so `sysctl -w` style writes don't fail; the value
        // we report stays fixed, which is fine for our purposes.
        let _ = path::create_file(p, 0o644, content.as_bytes().to_vec());
    }

    // A tiny /sys tree. nginx doesn't need it, but glibc-flavoured code probes
    // /sys/devices/system/cpu/online for the CPU count.
    let _ = path::create_file("/sys/devices/system/cpu/online", 0o444, b"0\n".to_vec());
    let _ = path::create_file("/sys/devices/system/cpu/present", 0o444, b"0\n".to_vec());
    let _ = path::create_file(
        "/sys/kernel/mm/transparent_hugepage/enabled",
        0o444,
        b"always [madvise] never\n".to_vec(),
    );
}
