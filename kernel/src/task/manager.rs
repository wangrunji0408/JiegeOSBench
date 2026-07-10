/// 任务管理器
/// 管理所有进程，提供调度接口

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use lazy_static::lazy_static;

use crate::arch::context::TaskContext;
use super::process::{Task, TaskState, Pid};

pub struct TaskManager {
    /// 所有任务
    pub tasks: BTreeMap<Pid, Arc<Mutex<Task>>>,
    /// 就绪队列
    pub ready_queue: VecDeque<Pid>,
    /// 当前运行的任务
    pub current: Option<Pid>,
    /// 空闲任务上下文
    pub idle_context: TaskContext,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            ready_queue: VecDeque::new(),
            current: None,
            idle_context: TaskContext::zero(),
        }
    }

    pub fn add_task(&mut self, task: Arc<Mutex<Task>>) {
        let pid = task.lock().pid;
        self.tasks.insert(pid, task);
        self.ready_queue.push_back(pid);
    }

    pub fn pick_next(&mut self) -> Option<Pid> {
        // 先检查sleeping任务是否到时间了
        let now = crate::timer::get_time_ms();
        let sleeping_pids: Vec<Pid> = self.tasks.iter()
            .filter_map(|(pid, task)| {
                let t = task.lock();
                if let TaskState::Sleeping(until) = t.state {
                    if now >= until { Some(*pid) } else { None }
                } else {
                    None
                }
            })
            .collect();

        for pid in sleeping_pids {
            if let Some(task) = self.tasks.get(&pid) {
                task.lock().state = TaskState::Ready;
                self.ready_queue.push_back(pid);
            }
        }

        // 从就绪队列取一个任务
        while let Some(pid) = self.ready_queue.pop_front() {
            if let Some(task) = self.tasks.get(&pid) {
                let state = task.lock().state;
                if state == TaskState::Ready {
                    return Some(pid);
                }
            }
        }
        None
    }

    pub fn current_task(&self) -> Option<Arc<Mutex<Task>>> {
        self.current.and_then(|pid| self.tasks.get(&pid).cloned())
    }

    pub fn wake_io_tasks(&mut self) {
        let pids: Vec<Pid> = self.tasks.keys().cloned().collect();
        for pid in pids {
            if let Some(task) = self.tasks.get(&pid) {
                let state = task.lock().state;
                if state == TaskState::Blocking {
                    task.lock().state = TaskState::Ready;
                    self.ready_queue.push_back(pid);
                }
            }
        }
    }

    pub fn exit_current(&mut self, code: i32) {
        if let Some(pid) = self.current {
            if let Some(task) = self.tasks.get(&pid) {
                task.lock().state = TaskState::Zombie(code);
                task.lock().exit_code = code;
            }
            self.current = None;
        }
    }
}

lazy_static! {
    pub static ref TASK_MANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::new());
}

pub fn init() {
    // 任务管理器已通过lazy_static初始化
}

pub fn add_task(task: Arc<Mutex<Task>>) {
    TASK_MANAGER.lock().add_task(task);
}

pub fn current_task() -> Option<Arc<Mutex<Task>>> {
    TASK_MANAGER.lock().current_task()
}

pub fn current_task_exit(code: i32) -> ! {
    {
        let mut mgr = TASK_MANAGER.lock();
        mgr.exit_current(code);
    }
    super::scheduler::schedule();
    unreachable!()
}
