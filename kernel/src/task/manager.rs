//! Ready queue of runnable tasks.

use super::task::TaskControlBlock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl TaskManager {
    const fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }
}

static TASK_MANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::new());

pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.lock().add(task);
}

pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.lock().fetch()
}
