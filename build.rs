use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn append_tree(root: &Path, directory: &Path, output: &mut Vec<u8>) {
    let mut children: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    for path in children {
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            append_tree(root, &path, output);
            continue;
        }
        let name = path.strip_prefix(root).unwrap().to_string_lossy();
        if name.starts_with('.') || name.contains("/.SIGN") {
            continue;
        }
        let (kind, data) = if metadata.file_type().is_symlink() {
            (
                2u8,
                fs::read_link(&path)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec(),
            )
        } else if metadata.is_file() {
            (1u8, fs::read(&path).unwrap())
        } else {
            continue;
        };
        let name = format!("/{name}");
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() <= u16::MAX as usize);
        output.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        output.push(kind);
        output.push(0);
        output.extend_from_slice(&(data.len() as u64).to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(&data);
    }
}

fn main() {
    println!("cargo:rerun-if-changed=user/init.S");
    println!("cargo:rerun-if-changed=assets/nginx.conf");
    println!("cargo:rerun-if-changed=assets/index.html");
    if !Path::new("rootfs/usr/sbin/nginx").exists() {
        let status = Command::new("sh")
            .arg("scripts/fetch-rootfs.sh")
            .status()
            .unwrap();
        assert!(status.success(), "failed to fetch Alpine nginx rootfs");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("init");
    let status = Command::new("zig")
        .args([
            "cc",
            "-target",
            "riscv64-linux-musl",
            "-nostdlib",
            "-static",
            "-fno-pie",
            "-no-pie",
            "-Wl,--build-id=none",
            "-o",
        ])
        .arg(&out)
        .arg("user/init.S")
        .status()
        .expect("failed to execute zig; install zig to build the Linux ABI test program");
    assert!(status.success(), "failed to build user/init.S");

    let mut archive = b"JIEGEFS1".to_vec();
    append_tree(Path::new("rootfs"), Path::new("rootfs"), &mut archive);
    archive.extend_from_slice(&0u16.to_le_bytes());
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("rootfs.jgfs"),
        archive,
    )
    .unwrap();
}
