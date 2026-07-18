use super::File;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

pub struct EpollEntry {
    pub fd: i32,
    pub events: u32,
    pub data: u64,
    pub file: Arc<dyn File>,
}

pub struct EpollFile {
    pub entries: Mutex<Vec<EpollEntry>>,
}

impl EpollFile {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl File for EpollFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
