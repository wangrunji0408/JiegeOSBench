use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

struct Entry {
    name: Vec<u8>,
    mode: u32,
    data: Vec<u8>,
    directory: bool,
}

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");

    if let Err(error) = build_archive() {
        panic!("failed to build rootfs CPIO archive: {error}");
    }
}

fn build_archive() -> io::Result<()> {
    let root = PathBuf::from("rootfs");
    let mut entries = Vec::new();
    println!("cargo:rerun-if-changed={}", root.display());
    collect(&root, Path::new(""), &mut entries)?;
    entries.sort_by(|left, right| left.name.cmp(&right.name));

    let mut archive = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let inode = u32::try_from(index + 1)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "too many files"))?;
        append_entry(&mut archive, entry, inode)?;
    }

    let trailer = Entry {
        name: b"TRAILER!!!".to_vec(),
        mode: 0,
        data: Vec::new(),
        directory: false,
    };
    append_entry(&mut archive, &trailer, 0)?;

    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "OUT_DIR is not set")
    })?);
    fs::write(output.join("rootfs.cpio"), archive)
}

fn collect(directory: &Path, relative: &Path, entries: &mut Vec<Entry>) -> io::Result<()> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let path = child.path();
        println!("cargo:rerun-if-changed={}", path.display());

        let file_type = child.file_type()?;
        let metadata = fs::symlink_metadata(&path)?;
        let path = relative.join(child.file_name());
        let name = path.as_os_str().as_encoded_bytes().to_vec();
        if name.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "rootfs path contains a NUL byte",
            ));
        }

        let (data, directory) = if file_type.is_symlink() {
            (
                fs::read_link(child.path())?
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
                false,
            )
        } else if file_type.is_dir() {
            (Vec::new(), true)
        } else if file_type.is_file() {
            (fs::read(child.path())?, false)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported rootfs file type: {}", child.path().display()),
            ));
        };

        entries.push(Entry {
            name,
            mode: metadata.mode(),
            data,
            directory,
        });

        if directory {
            collect(&child.path(), &path, entries)?;
        }
    }
    Ok(())
}

fn append_entry(archive: &mut Vec<u8>, entry: &Entry, inode: u32) -> io::Result<()> {
    let size = u32::try_from(entry.data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is too large for newc"))?;
    let name_size = u32::try_from(entry.name.len() + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path is too long"))?;
    let nlink = if entry.directory { 2 } else { 1 };
    let header = format!(
        concat!(
            "070701",
            "{inode:08x}",
            "{mode:08x}",
            "{uid:08x}",
            "{gid:08x}",
            "{nlink:08x}",
            "{mtime:08x}",
            "{size:08x}",
            "{dev_major:08x}",
            "{dev_minor:08x}",
            "{rdev_major:08x}",
            "{rdev_minor:08x}",
            "{name_size:08x}",
            "{check:08x}"
        ),
        inode = inode,
        mode = entry.mode,
        uid = 0,
        gid = 0,
        nlink = nlink,
        mtime = 0,
        size = size,
        dev_major = 0,
        dev_minor = 0,
        rdev_major = 0,
        rdev_minor = 0,
        name_size = name_size,
        check = 0,
    );
    archive.extend_from_slice(header.as_bytes());
    archive.extend_from_slice(&entry.name);
    archive.push(0);
    pad_to_four(archive);
    archive.extend_from_slice(&entry.data);
    pad_to_four(archive);
    Ok(())
}

fn pad_to_four(archive: &mut Vec<u8>) {
    while archive.len() % 4 != 0 {
        archive.push(0);
    }
}
