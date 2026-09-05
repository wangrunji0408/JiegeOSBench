use std::{env, fs, path::Path};
fn walk(dir: &Path, root: &Path, out: &mut String) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_symlink() {
            continue;
        }
        if p.is_dir() {
            walk(&p, root, out);
        } else {
            let name = format!("/{}", p.strip_prefix(root).unwrap().display());
            let abs = p.canonicalize().unwrap();
            out.push_str(&format!("({:?}, include_bytes!({:?})),\n", name, abs));
        }
    }
}
fn main() {
    println!("cargo:rerun-if-changed=rootfs");
    println!("cargo:rerun-if-changed=linker.ld");
    let mut s = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    walk(Path::new("rootfs"), Path::new("rootfs"), &mut s);
    s.push_str("];\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("rootfs.rs"),
        s,
    )
    .unwrap();
}
