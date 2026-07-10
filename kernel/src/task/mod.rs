pub mod elf;
pub mod manager;
pub mod process;
pub mod scheduler;

pub use process::{Task, TaskState, Pid};
pub use manager::{TASK_MANAGER, current_task, current_task_exit, add_task};
pub use scheduler::schedule;

use alloc::sync::Arc;
use spin::Mutex;

/// 初始化任务系统，创建init进程
pub fn init() {
    manager::init();
    // 创建init进程（运行/sbin/init 或 /usr/sbin/nginx）
    let init_elf = load_init_program();
    let init_task = process::Task::new_init(init_elf);
    manager::add_task(init_task);

    // 开始调度
    println!("[task] Starting scheduler...");
    scheduler::run();
}

fn load_init_program() -> &'static [u8] {
    // 调试：检查文件系统
    if let Some(entries) = crate::fs::FS.readdir("/usr") {
        println!("[task] /usr contents ({} items):", entries.len());
        for (name, _) in entries.iter().take(5) {
            println!("[task]   /usr/{}", name);
        }
    } else {
        println!("[task] /usr not found or not a directory");
    }

    let paths = ["/sbin/init", "/bin/init", "/usr/sbin/nginx", "/nginx"];
    for path in &paths {
        if let Some(node) = crate::fs::FS.lookup(path) {
            let node = node.lock();
            if let crate::fs::ramfs::INodeKind::File(data) = &node.kind {
                let data = data.lock();
                if !data.is_empty() {
                    println!("[task] Found init program: {}", path);
                    let boxed = alloc::boxed::Box::new(data.clone());
                    return alloc::boxed::Box::leak(boxed);
                }
            }
        }
    }
    panic!("No init program found! Please provide an initramfs with nginx or init.");
}

/// 唤醒等待IO的任务
pub fn wake_io_tasks() {
    manager::TASK_MANAGER.lock().wake_io_tasks();
}
