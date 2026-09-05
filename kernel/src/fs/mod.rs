pub mod cpio;
pub mod devices;
pub mod epoll;
pub mod eventfd;
pub mod fdtable;
pub mod file;
pub mod pipe;
pub mod vfs;

use alloc::string::String;
use alloc::sync::Arc;

use crate::abi::*;
use file::{DirFile, File, FileOps, NodeFile, RegFile};
use vfs::{Dentry, NodeKind};

/// Open a path relative to `base`, applying O_CREAT/O_EXCL/O_TRUNC/O_DIRECTORY.
pub fn open(base: &Arc<Dentry>, path: &str, flags: u32, mode: u32) -> Result<Arc<File>, i32> {
    // /dev/std* and /proc/self/fd/N re-open one of our own descriptors.
    let fd_alias = match path {
        "/dev/stdin" | "/proc/self/fd/0" => Some(0),
        "/dev/stdout" | "/proc/self/fd/1" => Some(1),
        "/dev/stderr" | "/proc/self/fd/2" => Some(2),
        p if p.starts_with("/proc/self/fd/") => p["/proc/self/fd/".len()..].parse::<i32>().ok(),
        _ => None,
    };
    if let Some(fd) = fd_alias {
        if let Some(task) = crate::task::try_current() {
            let f = task.fds().lock().get(fd)?;
            let keep = f.flags() & O_ACCMODE;
            let acc = flags & O_ACCMODE;
            let acc = if f.ops.dentry().map(|d| d.is_file()).unwrap_or(false) { acc } else { keep.max(acc) };
            return Ok(File::new(
                f.ops.clone(),
                (flags & !O_ACCMODE) & !(O_CREAT | O_EXCL | O_TRUNC) | acc,
                String::from(path),
            ));
        }
    }
    let follow = flags & O_NOFOLLOW == 0;
    let dentry = match vfs::lookup(base, path, follow) {
        Ok(d) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return Err(EEXIST);
            }
            d
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            let (parent, name) = vfs::lookup_parent(base, path)?;
            let umask = crate::task::current().inner.lock().umask;
            let f = Dentry::new_file(&name, Arc::downgrade(&parent), mode & !umask & 0o7777, alloc::vec::Vec::new());
            parent.add_child(f.clone())?;
            f
        }
        Err(e) => return Err(e),
    };
    open_dentry(dentry, flags)
}

pub fn open_dentry(dentry: Arc<Dentry>, flags: u32) -> Result<Arc<File>, i32> {
    let path = dentry.path();
    let acc = flags & O_ACCMODE;
    if flags & O_PATH != 0 {
        return Ok(File::new(
            Arc::new(NodeFile { dentry }),
            flags & (O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC),
            path,
        ));
    }
    let ops: Arc<dyn FileOps> = match &dentry.kind {
        NodeKind::Dir(_) => {
            if acc != O_RDONLY {
                return Err(EISDIR);
            }
            Arc::new(DirFile { dentry: dentry.clone() })
        }
        NodeKind::File(_) => {
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            if flags & O_TRUNC != 0 && acc != O_RDONLY {
                if let NodeKind::File(d) = &dentry.kind {
                    d.lock().clear();
                }
            }
            Arc::new(RegFile { dentry: dentry.clone() })
        }
        NodeKind::Symlink(_) => {
            // O_NOFOLLOW on a symlink without O_PATH
            return Err(ELOOP);
        }
        NodeKind::CharDev(major, minor) => {
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            devices::open_chardev(&dentry, *major, *minor)?
        }
        NodeKind::Fifo | NodeKind::Socket => return Err(ENXIO),
    };
    Ok(File::new(ops, flags & !(O_CREAT | O_EXCL | O_TRUNC | O_NOCTTY), path))
}

/// Create /dev nodes and the other directories nginx expects.
pub fn init_devfs() {
    use devices::*;
    vfs::mkdir_p("/dev");
    vfs::mkdir_p("/dev/shm");
    vfs::mkdir_p("/proc");
    vfs::mkdir_p("/sys");
    vfs::mkdir_p("/tmp");
    vfs::mkdir_p("/run");
    vfs::mkdir_p("/var/run");
    vfs::mkdir_p("/var/log/nginx");
    vfs::mkdir_p("/var/lib/nginx/tmp");
    vfs::mkdir_p("/var/tmp");
    vfs::mkdir_p("/root");
    vfs::mkdir_p("/etc");
    let mk = |path: &str, major: u32, minor: u32, mode: u32| {
        if vfs::lookup(&vfs::root(), path, false).is_ok() {
            return;
        }
        let d = vfs::create_node(path, S_IFCHR | mode, NodeKind::CharDev(major, minor)).unwrap();
        d.meta.lock().rdev = ((major as u64) << 8) | minor as u64;
    };
    mk("/dev/null", MAJOR_MEM, MINOR_NULL, 0o666);
    mk("/dev/zero", MAJOR_MEM, MINOR_ZERO, 0o666);
    mk("/dev/random", MAJOR_MEM, MINOR_RANDOM, 0o666);
    mk("/dev/urandom", MAJOR_MEM, MINOR_URANDOM, 0o666);
    mk("/dev/console", MAJOR_TTY, MINOR_CONSOLE, 0o620);
    mk("/dev/tty", MAJOR_TTY, 0, 0o666);
    mk("/dev/ttyS0", 4, 64, 0o620);
    mk("/dev/strace", 250, 0, 0o600);
    let _ = vfs::create_node("/dev/stdin", S_IFLNK | 0o777, NodeKind::Symlink(String::from("/proc/self/fd/0")));
    let _ = vfs::create_node("/dev/stdout", S_IFLNK | 0o777, NodeKind::Symlink(String::from("/proc/self/fd/1")));
    let _ = vfs::create_node("/dev/stderr", S_IFLNK | 0o777, NodeKind::Symlink(String::from("/proc/self/fd/2")));
    let _ = vfs::create_node("/dev/fd", S_IFLNK | 0o777, NodeKind::Symlink(String::from("/proc/self/fd")));
}

/// Stat helper used by fstatat: follows the dentry kind.
pub fn stat_path(base: &Arc<Dentry>, path: &str, follow: bool) -> Result<Stat, i32> {
    let d = vfs::lookup(base, path, follow)?;
    Ok(d.stat())
}

pub fn init(rootfs_addr: usize) -> usize {
    vfs::init_root();
    devices::init();
    let end = match cpio::load(rootfs_addr) {
        Ok(end) => end,
        Err(e) => {
            klog!("rootfs: {} — starting with an empty root", e);
            rootfs_addr
        }
    };
    init_devfs();
    klog!("rootfs: {} nodes loaded", cpio::count_files(&vfs::root()));
    end
}
