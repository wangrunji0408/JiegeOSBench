use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub struct Inode {
    pub is_dir: bool,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, Arc<Mutex<Inode>>>,
}

impl Inode {
    fn new_file() -> Arc<Mutex<Inode>> {
        Arc::new(Mutex::new(Inode {
            is_dir: false,
            data: Vec::new(),
            children: BTreeMap::new(),
        }))
    }
    fn new_dir() -> Arc<Mutex<Inode>> {
        Arc::new(Mutex::new(Inode {
            is_dir: true,
            data: Vec::new(),
            children: BTreeMap::new(),
        }))
    }
}

static ROOT: Mutex<Option<Arc<Mutex<Inode>>>> = Mutex::new(None);

pub fn init() {
    *ROOT.lock() = Some(Inode::new_dir());
}

fn root() -> Arc<Mutex<Inode>> {
    ROOT.lock().as_ref().unwrap().clone()
}

fn split(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty() && *s != ".").collect()
}

/// Look up an absolute path.
pub fn lookup(path: &str) -> Option<Arc<Mutex<Inode>>> {
    let mut cur = root();
    for comp in split(path) {
        let next = {
            let node = cur.lock();
            if !node.is_dir {
                return None;
            }
            node.children.get(comp).cloned()
        };
        cur = next?;
    }
    Some(cur)
}

/// Create all directories along `path`.
pub fn mkdir_p(path: &str) -> Arc<Mutex<Inode>> {
    let mut cur = root();
    for comp in split(path) {
        let next = {
            let mut node = cur.lock();
            node.children
                .entry(comp.to_string())
                .or_insert_with(Inode::new_dir)
                .clone()
        };
        cur = next;
    }
    cur
}

/// Create (or truncate) a regular file, creating parent dirs as needed.
pub fn create_file(path: &str) -> Option<Arc<Mutex<Inode>>> {
    let (dir, name) = split_parent(path)?;
    let parent = mkdir_p(&dir);
    let mut p = parent.lock();
    let file = Inode::new_file();
    p.children.insert(name, file.clone());
    Some(file)
}

fn split_parent(path: &str) -> Option<(String, String)> {
    let comps = split(path);
    if comps.is_empty() {
        return None;
    }
    let name = comps[comps.len() - 1].to_string();
    let dir = comps[..comps.len() - 1].join("/");
    Some((dir, name))
}

/// Write file contents, creating the file (and parents) if needed.
pub fn write_file(path: &str, contents: &[u8]) {
    let file = lookup(path).unwrap_or_else(|| create_file(path).unwrap());
    let mut f = file.lock();
    f.is_dir = false;
    f.data = contents.to_vec();
}

pub fn dir_entries(node: &Arc<Mutex<Inode>>) -> Vec<(String, bool)> {
    let n = node.lock();
    n.children
        .iter()
        .map(|(k, v)| (k.clone(), v.lock().is_dir))
        .collect()
}
